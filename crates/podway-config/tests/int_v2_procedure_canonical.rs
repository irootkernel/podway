//! V2MOD-007: Canonical JSON/IR and digest for a validated Procedure v2 model (dossier sections
//! 12.1 and 13.3).
//!
//! Section 12.1 fixes the authority chain `source -> parsed model -> Canonical JSON/IR -> digest`,
//! and section 13.3 states the consequence a reviewer relies on: the digest attests canonical
//! semantics, not source bytes, so "formatting and comments never affect it". These tests hold both
//! halves of that claim at once — every non-semantic difference collapses to one digest, and every
//! semantic difference, including the order of an order-bearing array, produces a different one.
//!
//! Canonical form is rebuilt from the validated model, never normalized out of the source document,
//! so the shape is fixed by four rules: documented defaults are materialized, absent optionals and
//! empty optional collections are omitted, author-order-meaningful arrays keep author order, and
//! authoring maps stay JSON objects whose key order Canonical JSON v1 normalizes.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use podway_config::{
    ConfigError, ParsedProcedure, ProcedureDocumentFormat, ValidatedProcedureV2,
    parse_procedure_document, validate_procedure_v2,
};
use podway_core::{SOURCE_PROJECTION_MAX_CHARACTERS, verify_canonical_json_v1};
use serde_json::Value;

// ---------------------------------------------------------------------------------------------
// Stage helpers
// ---------------------------------------------------------------------------------------------

fn admit(text: &str, format: ProcedureDocumentFormat) -> Result<ValidatedProcedureV2, ConfigError> {
    match parse_procedure_document(text.as_bytes(), format) {
        Ok(ParsedProcedure::V2(parsed)) => validate_procedure_v2(parsed),
        Ok(ParsedProcedure::V1(_)) => panic!("expected v2 dispatch, got v1"),
        Err(error) => Err(error),
    }
}

fn accept(text: &str, format: ProcedureDocumentFormat) -> ValidatedProcedureV2 {
    admit(text, format).expect("document must parse and validate")
}

fn yaml(text: &str) -> ValidatedProcedureV2 {
    accept(text, ProcedureDocumentFormat::Yaml)
}

fn json(text: &str) -> ValidatedProcedureV2 {
    accept(text, ProcedureDocumentFormat::Json)
}

fn yaml_digest(text: &str) -> String {
    yaml(text).digest().as_str().to_owned()
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn fixture(relative: &str) -> Vec<u8> {
    let path = repo_root().join(relative);
    fs::read(&path)
        .unwrap_or_else(|error| panic!("failed to read fixture {}: {error}", path.display()))
}

/// Applies one textual edit to [`BASE_YAML`] and proves the edit actually landed, so a stale anchor
/// can never make a "digest changed" or "digest unchanged" assertion vacuous.
fn edited(from: &str, to: &str) -> String {
    let edited = BASE_YAML.replace(from, to);
    assert_ne!(edited, BASE_YAML, "edit anchor did not match: {from:?}");
    edited
}

// ---------------------------------------------------------------------------------------------
// Reference documents
//
// `BASE_YAML` exercises every canonical shape rule in one document: optional scalars present and
// absent, defaulted item fields omitted, order-bearing arrays (instructions, options, items,
// evidence_from, selected items, graph.nodes, allowed_targets), and all three authoring maps
// (node_definitions, routes, assessment outcomes).
// ---------------------------------------------------------------------------------------------

const BASE_YAML: &str = concat!(
    "schema: podway.procedure/v2\n",
    "id: canonical-base\n",
    "version: \"2\"\n",
    "name: Canonical base\n",
    "purpose: Exercise every canonical shape rule in one document.\n",
    "description: A canonical-form reference document.\n",
    "goal_tracking: true\n",
    "node_definitions:\n",
    "  work:\n",
    "    type: action\n",
    "    title: Do the work\n",
    "    intent: Record the result.\n",
    "    instructions:\n",
    "      - Read the brief.\n",
    "      - Record the outcome.\n",
    "    items:\n",
    "      - id: result\n",
    "        type: text\n",
    "        prompt: Record the result.\n",
    "        required: true\n",
    "      - id: findings\n",
    "        type: list\n",
    "        prompt: List the findings.\n",
    "        required: false\n",
    "  assess:\n",
    "    type: decision\n",
    "    title: Assess the goal\n",
    "    description: The goal assessment decision.\n",
    "    objective: Determine the goal outcome.\n",
    "    prompt: Which outcome applies?\n",
    "    evidence_guidance:\n",
    "      - Read the recorded result.\n",
    "    options:\n",
    "      - id: achieved\n",
    "        label: Achieved\n",
    "        criteria: Every criterion is met.\n",
    "      - id: not-achieved\n",
    "        label: Not achieved\n",
    "      - id: superseded\n",
    "        label: Superseded\n",
    "    reason:\n",
    "      required: true\n",
    "      prompt: Explain the assessment.\n",
    "    assessment:\n",
    "      target: session_goal\n",
    "      outcomes:\n",
    "        achieved: achieved\n",
    "        not-achieved: not_achieved\n",
    "        superseded: superseded\n",
    "graph:\n",
    "  entry: perform\n",
    "  nodes:\n",
    "    - id: perform\n",
    "      use: work\n",
    "      next: decide\n",
    "    - id: decide\n",
    "      use: assess\n",
    "      evidence_from:\n",
    "        - node: perform\n",
    "          items:\n",
    "            - result\n",
    "      routes:\n",
    "        achieved:\n",
    "          to: finish\n",
    "          effect: advance\n",
    "        not-achieved:\n",
    "          to: perform\n",
    "          effect: rework\n",
    "        superseded:\n",
    "          to: finish\n",
    "          effect: advance\n",
    "    - id: finish\n",
    "      use: work\n",
    "      terminal: true\n",
    "manual_rework:\n",
    "  allowed_targets:\n",
    "    - perform\n",
);

/// The whole session-goal assessment block. Removing it makes the procedure legal without
/// `goal_tracking`, which is what the goal-tracking edit needs to vary one thing at a time.
const ASSESSMENT_BLOCK: &str = concat!(
    "    assessment:\n",
    "      target: session_goal\n",
    "      outcomes:\n",
    "        achieved: achieved\n",
    "        not-achieved: not_achieved\n",
    "        superseded: superseded\n",
);

/// One decision routing to two independent terminal actions. Swapping the two definitions or the
/// two terminal placements leaves every closed reference intact, which is what separates a map
/// reordering (no meaning) from an array reordering (meaning).
const BRANCH_YAML: &str = concat!(
    "schema: podway.procedure/v2\n",
    "id: branch\n",
    "version: \"1\"\n",
    "name: Branch\n",
    "purpose: Two independent terminals behind one decision.\n",
    "node_definitions:\n",
    "  pick:\n",
    "    type: decision\n",
    "    title: Pick\n",
    "    objective: Choose a branch.\n",
    "    prompt: Which branch?\n",
    "    options:\n",
    "      - id: left\n",
    "        label: Left\n",
    "      - id: right\n",
    "        label: Right\n",
    "    reason:\n",
    "      required: true\n",
    "  work:\n",
    "    type: action\n",
    "    title: Work\n",
    "    intent: Do the work.\n",
    "graph:\n",
    "  entry: start\n",
    "  nodes:\n",
    "    - id: start\n",
    "      use: pick\n",
    "      routes:\n",
    "        left:\n",
    "          to: alpha\n",
    "          effect: advance\n",
    "        right:\n",
    "          to: beta\n",
    "          effect: advance\n",
    "    - id: alpha\n",
    "      use: work\n",
    "      terminal: true\n",
    "    - id: beta\n",
    "      use: work\n",
    "      terminal: true\n",
);

const DEFINITION_PICK: &str = concat!(
    "  pick:\n",
    "    type: decision\n",
    "    title: Pick\n",
    "    objective: Choose a branch.\n",
    "    prompt: Which branch?\n",
    "    options:\n",
    "      - id: left\n",
    "        label: Left\n",
    "      - id: right\n",
    "        label: Right\n",
    "    reason:\n",
    "      required: true\n",
);
const DEFINITION_WORK: &str = concat!(
    "  work:\n",
    "    type: action\n",
    "    title: Work\n",
    "    intent: Do the work.\n",
);
const PLACEMENT_ALPHA: &str = "    - id: alpha\n      use: work\n      terminal: true\n";
const PLACEMENT_BETA: &str = "    - id: beta\n      use: work\n      terminal: true\n";

/// `BRANCH_YAML` as JSON, written with deliberately non-alphabetical keys throughout.
const BRANCH_JSON: &str = r#"{
  "schema": "podway.procedure/v2",
  "id": "branch",
  "version": "1",
  "name": "Branch",
  "purpose": "Two independent terminals behind one decision.",
  "node_definitions": {
    "pick": {
      "type": "decision",
      "title": "Pick",
      "objective": "Choose a branch.",
      "prompt": "Which branch?",
      "options": [{"id": "left", "label": "Left"}, {"id": "right", "label": "Right"}],
      "reason": {"required": true}
    },
    "work": {"type": "action", "title": "Work", "intent": "Do the work."}
  },
  "graph": {
    "entry": "start",
    "nodes": [
      {
        "id": "start",
        "use": "pick",
        "routes": {
          "left": {"to": "alpha", "effect": "advance"},
          "right": {"to": "beta", "effect": "advance"}
        }
      },
      {"id": "alpha", "use": "work", "terminal": true},
      {"id": "beta", "use": "work", "terminal": true}
    ]
  }
}"#;

/// The same JSON document with every object's keys written in a different order — including the
/// `node_definitions` and `routes` authoring maps, whose keys are swapped.
const BRANCH_JSON_REORDERED: &str = r#"{
  "graph": {
    "nodes": [
      {
        "routes": {
          "right": {"effect": "advance", "to": "beta"},
          "left": {"effect": "advance", "to": "alpha"}
        },
        "use": "pick",
        "id": "start"
      },
      {"terminal": true, "use": "work", "id": "alpha"},
      {"terminal": true, "use": "work", "id": "beta"}
    ],
    "entry": "start"
  },
  "node_definitions": {
    "work": {"intent": "Do the work.", "title": "Work", "type": "action"},
    "pick": {
      "reason": {"required": true},
      "options": [{"label": "Left", "id": "left"}, {"label": "Right", "id": "right"}],
      "prompt": "Which branch?",
      "objective": "Choose a branch.",
      "title": "Pick",
      "type": "decision"
    }
  },
  "purpose": "Two independent terminals behind one decision.",
  "name": "Branch",
  "version": "1",
  "id": "branch",
  "schema": "podway.procedure/v2"
}"#;

// ---------------------------------------------------------------------------------------------
// 1. Golden: the reviewable fixture pair has one canonical form and the contract digest
// ---------------------------------------------------------------------------------------------

/// The exact canonical bytes of `tests/fixtures/v2/procedures/equivalent-procedure.{yaml,json}`.
///
/// Every canonical rule is visible here: `min_length`/`max_length`/`multiline` are materialized on
/// the text item although the source omits two of them; the evidence reference carries an explicit
/// `required`; `description` and `goal_tracking` are present because the source declares them; no
/// empty collection appears anywhere; `node_definitions`, `routes`, and `outcomes` are objects with
/// byte-sorted keys, while `graph.nodes`, `options`, `items`, `instructions`, and `allowed_targets`
/// keep the order the author wrote.
const FIXTURE_CANONICAL_JSON: &str = r#"{"description":"A reviewable v2 known answer.","goal_tracking":true,"graph":{"entry":"perform","nodes":[{"id":"perform","next":"decide","use":"work"},{"evidence_from":[{"items":["result"],"node":"perform","required":true}],"id":"decide","routes":{"achieved":{"effect":"advance","to":"finish"},"not-achieved":{"effect":"advance","to":"finish"},"superseded":{"effect":"advance","to":"finish"}},"use":"assess"},{"id":"finish","terminal":true,"use":"work"}]},"id":"fixture-equivalence","manual_rework":{"allowed_targets":["perform"]},"name":"Fixture equivalence","node_definitions":{"assess":{"assessment":{"outcomes":{"achieved":"achieved","not-achieved":"not_achieved","superseded":"superseded"},"target":"session_goal"},"objective":"Determine the goal outcome.","options":[{"id":"achieved","label":"Achieved"},{"id":"not-achieved","label":"Not achieved"},{"id":"superseded","label":"Superseded"}],"prompt":"Which outcome applies?","reason":{"prompt":"Explain the assessment.","required":true},"title":"Assess the goal","type":"decision"},"work":{"instructions":["Work outside Podway."],"intent":"Record the result.","items":[{"id":"result","max_length":1000,"min_length":0,"multiline":true,"prompt":"Record the result.","required":true,"type":"text"}],"title":"Do the work","type":"action"}},"purpose":"Lock YAML and JSON to one structural Procedure value.","schema":"podway.procedure/v2","version":"2"}"#;

#[test]
fn the_fixture_pair_has_one_canonical_form_and_the_contract_digest() {
    let yaml_source = fixture("tests/fixtures/v2/procedures/equivalent-procedure.yaml");
    let json_source = fixture("tests/fixtures/v2/procedures/equivalent-procedure.json");

    let from_yaml = accept(
        std::str::from_utf8(&yaml_source).unwrap(),
        ProcedureDocumentFormat::Yaml,
    );
    let from_json = accept(
        std::str::from_utf8(&json_source).unwrap(),
        ProcedureDocumentFormat::Json,
    );

    assert_eq!(
        from_yaml.canonical_json().as_bytes(),
        from_json.canonical_json().as_bytes(),
    );
    assert_eq!(from_yaml.digest(), from_json.digest());
    assert_eq!(from_yaml.canonical_json().as_str(), FIXTURE_CANONICAL_JSON);

    // Closes the loop the fixture recipe promises: the digest the recipe pins is the digest this
    // implementation produces from the pair, not a digest of either source document's own bytes.
    let contract: Value = serde_json::from_slice(&fixture(
        "tests/fixtures/v2/procedures/equivalence-contract.json",
    ))
    .expect("the equivalence contract fixture is JSON");
    assert_eq!(
        contract["canonical_sha256"].as_str(),
        Some(from_yaml.digest().as_str()),
    );
}

// ---------------------------------------------------------------------------------------------
// 2. Ordering and formatting are not meaning
// ---------------------------------------------------------------------------------------------

#[test]
fn yaml_mapping_key_order_never_changes_the_digest() {
    let base = yaml_digest(BASE_YAML);

    // Scalar field order inside one node definition.
    assert_eq!(
        yaml_digest(&edited(
            "    type: action\n    title: Do the work\n    intent: Record the result.\n",
            "    intent: Record the result.\n    title: Do the work\n    type: action\n",
        )),
        base,
    );
    // An authoring map keyed by option id (assessment outcomes).
    assert_eq!(
        yaml_digest(&edited(
            "        achieved: achieved\n        not-achieved: not_achieved\n        superseded: superseded\n",
            "        superseded: superseded\n        achieved: achieved\n        not-achieved: not_achieved\n",
        )),
        base,
    );
    // An authoring map keyed by option id whose values are objects (decision routes).
    assert_eq!(
        yaml_digest(&edited(
            "        achieved:\n          to: finish\n          effect: advance\n        not-achieved:\n          to: perform\n          effect: rework\n",
            "        not-achieved:\n          to: perform\n          effect: rework\n        achieved:\n          to: finish\n          effect: advance\n",
        )),
        base,
    );
}

#[test]
fn json_mapping_key_order_never_changes_the_digest() {
    assert_eq!(
        json(BRANCH_JSON).canonical_json(),
        json(BRANCH_JSON_REORDERED).canonical_json(),
    );
    assert_eq!(
        json(BRANCH_JSON).digest(),
        json(BRANCH_JSON_REORDERED).digest(),
    );
}

#[test]
fn yaml_block_and_flow_style_produce_the_same_digest() {
    let flow = edited(
        concat!(
            "    options:\n",
            "      - id: achieved\n",
            "        label: Achieved\n",
            "        criteria: Every criterion is met.\n",
            "      - id: not-achieved\n",
            "        label: Not achieved\n",
            "      - id: superseded\n",
            "        label: Superseded\n",
        ),
        concat!(
            "    options: [{id: achieved, label: \"Achieved\", criteria: \"Every criterion is met.\"},",
            " {id: not-achieved, label: \"Not achieved\"},",
            " {id: superseded, label: \"Superseded\"}]\n",
        ),
    );
    assert_eq!(yaml_digest(&flow), yaml_digest(BASE_YAML));

    let flow_targets = edited(
        "  allowed_targets:\n    - perform\n",
        "  allowed_targets: [perform]\n",
    );
    assert_eq!(yaml_digest(&flow_targets), yaml_digest(BASE_YAML));
}

#[test]
fn yaml_comments_and_blank_lines_never_change_the_digest() {
    let commented = format!(
        "# Canonical form is derived from the model, so this comment cannot reach the digest.\n\n{}",
        edited(
            "    instructions:\n",
            "\n    # The order of these instructions is meaning; this comment is not.\n    instructions:\n",
        ),
    );
    assert_eq!(yaml_digest(&commented), yaml_digest(BASE_YAML));
}

// ---------------------------------------------------------------------------------------------
// 3. Authoring a documented default is not meaning
// ---------------------------------------------------------------------------------------------

#[test]
fn authored_defaults_and_omitted_defaults_produce_the_same_digest() {
    let base = yaml_digest(BASE_YAML);

    // Every text-item default, written out.
    assert_eq!(
        yaml_digest(&edited(
            "        prompt: Record the result.\n        required: true\n",
            "        prompt: Record the result.\n        required: true\n        min_length: 0\n        max_length: 4000\n        multiline: true\n",
        )),
        base,
    );
    // Every list-item default, written out.
    assert_eq!(
        yaml_digest(&edited(
            "        prompt: List the findings.\n        required: false\n",
            "        prompt: List the findings.\n        required: false\n        min_items: 0\n        max_items: 50\n        max_item_length: 500\n        unique: true\n",
        )),
        base,
    );
    // The evidence-reference `required` default.
    assert_eq!(
        yaml_digest(&edited(
            "        - node: perform\n          items:\n",
            "        - node: perform\n          required: true\n          items:\n",
        )),
        base,
    );
}

// ---------------------------------------------------------------------------------------------
// 4. Every semantic edit changes the digest
// ---------------------------------------------------------------------------------------------

#[test]
fn semantic_edits_change_the_digest() {
    let base = yaml_digest(BASE_YAML);
    let mut digests = BTreeSet::from([base.clone()]);

    let edits = [
        // An option label is reviewable meaning.
        edited(
            "        label: Not achieved\n",
            "        label: Not yet achieved\n",
        ),
        // A route retarget changes where the procedure goes.
        edited(
            "          to: perform\n          effect: rework\n",
            "          to: finish\n          effect: rework\n",
        ),
        // A different selected recorded item is different read-back.
        edited(
            "          items:\n            - result\n",
            "          items:\n            - findings\n",
        ),
        // A skip policy appears where there was none.
        edited(
            "      use: work\n      next: decide\n",
            "      use: work\n      skip:\n        allowed: true\n        reason_required: true\n      next: decide\n",
        ),
        // Array order is meaning: the instruction sequence is reversed.
        edited(
            "      - Read the brief.\n      - Record the outcome.\n",
            "      - Record the outcome.\n      - Read the brief.\n",
        ),
    ];
    for (index, edit) in edits.iter().enumerate() {
        let digest = yaml_digest(edit);
        assert_ne!(digest, base, "edit {index} must change the digest");
        assert!(digests.insert(digest), "edit {index} collided with another");
    }

    // `goal_tracking` is a procedure-level opt-in, so it is varied on a document that carries no
    // session-goal assessment: an assessment without the opt-in is invalid, which would change two
    // things at once.
    let tracked = BASE_YAML.replace(ASSESSMENT_BLOCK, "");
    assert_ne!(tracked, BASE_YAML);
    let untracked = tracked.replace("goal_tracking: true\n", "");
    assert_ne!(untracked, tracked);
    assert_ne!(yaml_digest(&untracked), yaml_digest(&tracked));
}

// ---------------------------------------------------------------------------------------------
// 5. A map reordering is not meaning; an array reordering is
// ---------------------------------------------------------------------------------------------

#[test]
fn node_definition_map_order_is_not_meaning_but_graph_node_array_order_is() {
    let base = yaml(BRANCH_YAML);

    let swapped_definitions = BRANCH_YAML.replace(
        &format!("{DEFINITION_PICK}{DEFINITION_WORK}"),
        &format!("{DEFINITION_WORK}{DEFINITION_PICK}"),
    );
    assert_ne!(swapped_definitions, BRANCH_YAML);
    let swapped_definitions = yaml(&swapped_definitions);
    assert_eq!(swapped_definitions.canonical_json(), base.canonical_json());
    assert_eq!(swapped_definitions.digest(), base.digest());

    let swapped_placements = BRANCH_YAML.replace(
        &format!("{PLACEMENT_ALPHA}{PLACEMENT_BETA}"),
        &format!("{PLACEMENT_BETA}{PLACEMENT_ALPHA}"),
    );
    assert_ne!(swapped_placements, BRANCH_YAML);
    // Both orders stay closed-reference-valid: the routes name placements, not positions.
    let swapped_placements = yaml(&swapped_placements);
    assert_ne!(swapped_placements.digest(), base.digest());
}

// ---------------------------------------------------------------------------------------------
// 6. The canonical source projection budget
// ---------------------------------------------------------------------------------------------

/// A document carrying `count` maximal choice items in one definition. Every filler item is
/// identical in width — fixed-width identifier, maximum-length help, maximum choice set — so the
/// item count is an exactly linear dial on the canonical projection size.
fn projection_document(count: usize) -> String {
    let choices: String = (0..32)
        .map(|index| format!("          - \"{:c<118}{index:02}\"\n", ""))
        .collect();
    let items: String = (0..count)
        .map(|index| {
            format!(
                "      - id: i{index:02}\n        type: choice\n        prompt: \"{}\"\n        help: \"{}\"\n        required: true\n        choices:\n{choices}",
                "p".repeat(300),
                "h".repeat(1_000),
            )
        })
        .collect();
    format!(
        "schema: podway.procedure/v2\nid: projection\nversion: \"1\"\nname: Projection\npurpose: Fill the canonical projection budget.\nnode_definitions:\n  bulk:\n    type: action\n    title: Bulk\n    intent: Hold the filler items.\n    items:\n{items}graph:\n  entry: n\n  nodes:\n    - id: n\n      use: bulk\n      terminal: true\n",
    )
}

fn projection_characters(count: usize) -> usize {
    yaml(&projection_document(count))
        .canonical_json()
        .as_str()
        .chars()
        .count()
}

#[test]
fn the_canonical_projection_budget_accepts_the_limit_and_rejects_one_filler_item_over() {
    let one = projection_characters(1);
    let step = projection_characters(2) - one;
    let at_limit = 1 + (SOURCE_PROJECTION_MAX_CHARACTERS - one) / step;

    let accepted = yaml(&projection_document(at_limit));
    let characters = accepted.canonical_json().as_str().chars().count();
    assert!(
        characters <= SOURCE_PROJECTION_MAX_CHARACTERS,
        "the at-limit document must fit: {characters}"
    );
    assert!(
        characters + step > SOURCE_PROJECTION_MAX_CHARACTERS,
        "the at-limit document must sit within one filler item of the budget: {characters}"
    );
    assert!(
        characters * 10 >= SOURCE_PROJECTION_MAX_CHARACTERS * 9,
        "the at-limit document must be within ten percent of the budget: {characters}"
    );

    let over = projection_document(at_limit + 1);
    let first = admit(&over, ProcedureDocumentFormat::Yaml).expect_err("one over must be rejected");
    let second =
        admit(&over, ProcedureDocumentFormat::Yaml).expect_err("one over must be rejected");
    assert_eq!(first, second, "the rejection must be deterministic");
    assert!(
        matches!(
            first,
            ConfigError::OutOfBounds {
                field: "canonical source projection",
                min: 1,
                max: SOURCE_PROJECTION_MAX_CHARACTERS,
                actual,
            } if actual > SOURCE_PROJECTION_MAX_CHARACTERS
        ),
        "{first:?}"
    );
}

// ---------------------------------------------------------------------------------------------
// 7. Canonical output properties
// ---------------------------------------------------------------------------------------------

#[test]
fn canonical_output_is_canonical_json_v1_and_re_canonicalizing_it_reaches_a_fixpoint() {
    for source in [BASE_YAML, BRANCH_YAML] {
        let validated = yaml(source);
        let canonical = validated.canonical_json().as_str().to_owned();
        assert_eq!(verify_canonical_json_v1(canonical.as_bytes()), Ok(()));

        // The canonical document is itself a valid Procedure v2 JSON document, and canonicalizing
        // it again is the identity — the property a formatter, a preview, and a confirmed digest
        // all rely on.
        let round_tripped = json(&canonical);
        assert_eq!(round_tripped.canonical_json().as_str(), canonical);
        assert_eq!(round_tripped.digest(), validated.digest());
    }
}

#[test]
fn canonical_form_omits_absent_optionals_and_empty_collections() {
    // `BRANCH_YAML` declares no description, no goal tracking, no instructions, no items, no
    // evidence, no skip policy, no criteria, no reason prompt, no assessment, and no manual rework.
    // The schema forbids an explicitly empty optional collection, so canonical form omits each key
    // outright rather than emitting `null`, `[]`, `{}`, or `false`.
    let canonical = yaml(BRANCH_YAML).canonical_json().as_str().to_owned();
    for absent in [
        "description",
        "goal_tracking",
        "instructions",
        "items",
        "evidence_from",
        "skip",
        "criteria",
        "assessment",
        "manual_rework",
    ] {
        assert!(
            !canonical.contains(&format!("\"{absent}\"")),
            "canonical form must omit `{absent}`: {canonical}"
        );
    }
    assert!(!canonical.contains("null"));
    assert!(!canonical.contains("[]"));
    assert!(!canonical.contains("{}"));
}
