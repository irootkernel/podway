//! V2AUT-007: Procedure v1 → v2 conversion (`podway procedure convert`).
//!
//! Four things need proving about a converter, and they are different in kind:
//!
//! - **It produces a document the rest of the toolchain accepts.** Not "it validates": the
//!   converted text is put back through parse, validate, the canonical emitter, and the aggregate
//!   gate, and the gate must report *nothing at all* — no error and no advisory — because
//!   `podway procedure convert x.yaml > y.yaml && podway procedure check --warnings-as-errors y.yaml`
//!   is the documented workflow. That single assertion validates the mapping and every lint
//!   threshold at once against real shipped content.
//! - **It is deterministic.** Byte identity across a hundred conversions, and byte identity between
//!   the YAML and JSON encodings of the same v1 document, because convert always emits YAML.
//! - **Its meaning is pinned.** The converted digest of each shipped preset is a golden literal, so
//!   any change to the mapping — a materialized default, a synthesized string, an omitted field —
//!   fails here instead of silently changing what a converted procedure asks operators to do.
//! - **It refuses rather than truncates.** v2 tightened many v1 bounds. Every class of value that
//!   can be legal in v1 and illegal in v2 has a negative fixture asserting the exact diagnostic,
//!   and the bound constants the converter pre-checks with are pinned against the v2 constructors
//!   that actually enforce them, so the two cannot drift.
//!
//! **A shipped preset does not convert, and that is a finding, not a test bug.**
//! `assets/presets/analysis.yaml` declares `max_items: 200` on one list item; Procedure v2 caps a
//! list at 100 entries (`assets/schemas/procedure-v2.schema.json` `$defs/list_item`). It is
//! asserted below as the overflow it is. Converting it would require editing a digest-locked
//! contract asset, which is a product decision and not this task's to make.

use std::path::{Path, PathBuf};

use podway_config::{
    AuthoringContext, AuthoringStage, ConvertedProcedureV2, FormatRequest, ParsedNodeDefinition,
    ParsedProcedure, ProcedureDocumentFormat, ValidatedProcedureV1, ValidatedProcedureV2,
    check_procedure_v2, convert_procedure_v1_to_v2, finalize_diagnostics, format_procedure_v2,
    parse_procedure_document, parse_procedure_v1, validate_procedure_v2,
};
use podway_core::{AuthoringDiagnostic, AuthoringSeverity, GraphPlacementV2};
use serde_json::Value;

// ---------------------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------------------

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The shipped presets that convert. `analysis` is the fourth and is covered by
/// [`v2aut007_the_analysis_preset_does_not_convert_because_v2_caps_a_list_at_one_hundred`].
const CONVERTIBLE_PRESETS: [&str; 3] = ["bug-fix", "docs-only", "sw-dev"];

fn preset_source(name: &str) -> String {
    std::fs::read_to_string(repo_root().join(format!("assets/presets/{name}.yaml")))
        .expect("a shipped preset must be readable")
}

fn admit_v1(source: &str) -> ValidatedProcedureV1 {
    parse_procedure_v1(source, ProcedureDocumentFormat::Yaml)
        .expect("the v1 fixture must parse and validate")
}

/// Converts `source` as a v1 YAML document, reporting the diagnostics on failure.
fn convert(source: &str) -> Result<ConvertedProcedureV2, Vec<AuthoringDiagnostic>> {
    let validated = admit_v1(source);
    let context = AuthoringContext::new("workflow.yaml", source, ProcedureDocumentFormat::Yaml);
    convert_procedure_v1_to_v2(&validated, &context)
}

fn converted(source: &str) -> ConvertedProcedureV2 {
    match convert(source) {
        Ok(converted) => converted,
        Err(diagnostics) => panic!("the fixture must convert, got {diagnostics:#?}"),
    }
}

/// The overflow report in the shape a caller reports it: sorted and bounded by the shared
/// finalizer, exactly as the CLI does, rendered as `(code, field, message)`.
fn overflow_report(source: &str) -> Vec<(&'static str, String, String)> {
    let diagnostics = convert(source).err().unwrap_or_default();
    let report = finalize_diagnostics(
        diagnostics
            .into_iter()
            .map(|diagnostic| (AuthoringStage::Validate, diagnostic))
            .collect(),
    );
    report
        .diagnostics()
        .iter()
        .map(|diagnostic| {
            assert_eq!(
                diagnostic.severity(),
                AuthoringSeverity::Error,
                "every conversion finding is a refusal, never advice"
            );
            (
                diagnostic.code().as_str(),
                diagnostic.field().to_owned(),
                diagnostic.message().to_owned(),
            )
        })
        .collect()
}

/// The `(code, field)` pairs of an overflow report.
fn overflow_codes(source: &str) -> Vec<(&'static str, String)> {
    overflow_report(source)
        .into_iter()
        .map(|(code, field, _)| (code, field))
        .collect()
}

fn admit_v2(document: &str) -> ValidatedProcedureV2 {
    match parse_procedure_document(document.as_bytes(), ProcedureDocumentFormat::Yaml) {
        Ok(ParsedProcedure::V2(parsed)) => {
            validate_procedure_v2(parsed).expect("a converted document must validate as v2")
        }
        Ok(ParsedProcedure::V1(_)) => panic!("a converted document declares the v2 schema"),
        Err(error) => panic!("a converted document must parse: {error}\n{document}"),
    }
}

// ---------------------------------------------------------------------------------------------
// The v1 fixture builder
// ---------------------------------------------------------------------------------------------

/// The smallest legal v1 document, with one text item, parameterized by whatever a test varies.
///
/// Written as text rather than as a `ProcedureDefinitionV1` literal because the converter's input
/// is a *parsed* document: building the struct directly would bypass `serde`'s default
/// materialization, which is the very thing the mapping relies on.
fn v1_document(description: Option<&str>, stages: &str, rework: &str) -> String {
    let description = description.map_or_else(String::new, |text| format!("description: {text}\n"));
    format!(
        "schema: podway.procedure/v1\nid: fixture\nversion: \"1\"\nname: Fixture\n{description}stages:\n{stages}rework:\n  allow_return_to: {rework}\n"
    )
}

/// A one-stage document whose single item is `item`, indented to sit under `items:`.
fn v1_with_item(item: &str) -> String {
    v1_document(
        None,
        &format!("  - id: only\n    title: Only stage\n    items:\n{item}"),
        "[only]",
    )
}

// ---------------------------------------------------------------------------------------------
// The shipped presets
// ---------------------------------------------------------------------------------------------

/// The converted digest of every preset that converts.
///
/// Golden literals, and the reason they are golden rather than recomputed: a recomputed digest
/// asserts that the converter equals itself. These pin what a converted procedure *means*, so a
/// mapping change — dropping a materialized default, rewording a synthesized intent, reordering
/// rework targets — fails here and has to be justified rather than absorbed.
const CONVERTED_PRESET_DIGESTS: [(&str, &str); 3] = [
    (
        "bug-fix",
        "sha256:771ab15301b316494cdd2b3cfaa17f50308713bb090bd0bd466ac2670d683f05",
    ),
    (
        "docs-only",
        "sha256:b50e59b3a88791d6e0e01e6b8e2a4b6b6cee4bae32b667b65ecdaca40acae6cb",
    ),
    (
        "sw-dev",
        "sha256:73162fc5af10fe2a83fea752dd551e6b7cc823e06a2dd33aee4b08f9a16442bc",
    ),
];

#[test]
fn v2aut007_every_convertible_preset_converts_and_pins_both_digests() {
    for (preset, digest) in CONVERTED_PRESET_DIGESTS {
        let source = preset_source(preset);
        let validated = admit_v1(&source);
        let converted = converted(&source);

        assert_eq!(converted.digest().as_str(), digest, "{preset}");
        assert_eq!(
            converted.source_digest(),
            validated.digest(),
            "{preset}: the reported source digest is the v1 document's own digest, so a reviewer \
             can pin both ends of the conversion"
        );
        assert_eq!(
            admit_v2(converted.document()).digest(),
            converted.digest(),
            "{preset}: the advertised digest must be the digest the emitted text actually has"
        );
    }
}

/// The roadmap gate: what convert writes is what the rest of the toolchain accepts, silently.
#[test]
fn v2aut007_a_converted_preset_is_canonical_and_the_aggregate_gate_reports_nothing() {
    for preset in CONVERTIBLE_PRESETS {
        let source = preset_source(preset);
        let converted = converted(&source);
        let request = FormatRequest {
            source: converted.document(),
            source_path: "converted.yaml",
            format: ProcedureDocumentFormat::Yaml,
        };

        let formatted = format_procedure_v2(request)
            .unwrap_or_else(|failure| panic!("{preset}: the candidate must format: {failure:?}"));
        assert_eq!(
            formatted.document(),
            converted.document(),
            "{preset}: a converted document is a fixpoint of the canonical emitter, comments \
             included, so `format --check` on it exits 0"
        );
        assert!(!formatted.changed(), "{preset}");

        // Not "no errors": nothing at all. `--warnings-as-errors` moves the exit code whenever any
        // finding is present, so one advisory here would break the documented convert-then-check
        // pipeline. This is simultaneously the mapping's proof and every lint threshold's proof
        // against real content.
        let report = check_procedure_v2(request);
        assert_eq!(
            report.diagnostics(),
            &[],
            "{preset}: a converted preset must be clean under every authoring stage"
        );
        assert_eq!(report.total(), 0, "{preset}");
        assert!(report.valid(), "{preset}");
    }
}

#[test]
fn v2aut007_converting_a_preset_a_hundred_times_produces_identical_bytes() {
    for preset in CONVERTIBLE_PRESETS {
        let source = preset_source(preset);
        let baseline = converted(&source);
        for _ in 0..100 {
            let repeat = converted(&source);
            assert_eq!(repeat.document(), baseline.document(), "{preset}");
            assert_eq!(repeat.digest(), baseline.digest(), "{preset}");
            assert_eq!(repeat.source_digest(), baseline.source_digest(), "{preset}");
        }
    }
}

/// A v1 document's encoding is not part of its meaning, and convert always emits YAML — so the JSON
/// and YAML forms of one procedure must produce the same candidate, byte for byte.
#[test]
fn v2aut007_the_json_and_yaml_encodings_of_one_v1_document_convert_identically() {
    let yaml_source = preset_source("sw-dev");
    let document: Value =
        serde_yaml::from_str(&yaml_source).expect("the preset must decode as a value");
    let json_source = serde_json::to_string_pretty(&document).expect("a v1 document re-encodes");
    assert_ne!(
        json_source, yaml_source,
        "the two encodings must actually differ, or the parity claim is vacuous"
    );

    let json_validated = parse_procedure_v1(&json_source, ProcedureDocumentFormat::Json)
        .expect("the JSON encoding must parse as v1");
    let json_context =
        AuthoringContext::new("sw-dev.json", &json_source, ProcedureDocumentFormat::Json);
    let from_json = convert_procedure_v1_to_v2(&json_validated, &json_context)
        .expect("the JSON encoding must convert");
    let from_yaml = converted(&yaml_source);

    assert_eq!(from_json.document(), from_yaml.document());
    assert_eq!(from_json.digest(), from_yaml.digest());
    assert_eq!(
        from_json.source_digest(),
        from_yaml.source_digest(),
        "the v1 digest is canonical over the model, so it is already encoding-independent"
    );
}

/// The fourth preset, and the reason it is not in [`CONVERTIBLE_PRESETS`].
///
/// `assets/presets/analysis.yaml` declares a list item with `max_items: 200`. Procedure v2 caps a
/// list at 100 entries. This is a real product finding rather than a fixture defect, so it is
/// asserted as the exact refusal it produces: silently rewriting the shipped preset to make a test
/// pass would change a digest-locked contract asset.
#[test]
fn v2aut007_the_analysis_preset_does_not_convert_because_v2_caps_a_list_at_one_hundred() {
    let report = overflow_report(&preset_source("analysis"));
    assert_eq!(
        report,
        vec![(
            "AUTHORING_SCHEMA_INVALID",
            "stages[collect-sources].items[sources].max_items".to_owned(),
            "The v1 list item allows up to 200 entries, over the Procedure v2 maximum of 100."
                .to_owned(),
        )],
        "the only obstacle is the one oversized bound, reported against the v1 path that carries it"
    );
}

// ---------------------------------------------------------------------------------------------
// Structural mapping
// ---------------------------------------------------------------------------------------------

#[test]
fn v2aut007_stages_become_action_definitions_placed_in_a_linear_chain() {
    let document = converted(&preset_source("sw-dev"));
    let parsed = admit_v2(document.document());
    let parsed = parsed.parsed();

    let stage_ids: Vec<&str> = parsed
        .node_definitions()
        .iter()
        .map(|definition| definition.id().as_str())
        .collect();
    assert_eq!(
        stage_ids,
        [
            "understand",
            "inspect",
            "plan",
            "implement",
            "verify",
            "review",
            "finish"
        ],
        "one definition per stage, keyed by the stage identifier, in stage order"
    );
    assert!(
        parsed
            .node_definitions()
            .iter()
            .all(|definition| matches!(definition, ParsedNodeDefinition::Action(_))),
        "a conversion never invents a decision"
    );

    assert_eq!(parsed.graph().entry().as_str(), "understand");
    let placements: Vec<(&str, &str, Option<&str>, bool)> = parsed
        .graph()
        .placements()
        .iter()
        .map(|placement| match placement {
            GraphPlacementV2::Action(action) => (
                action.id().as_str(),
                action.definition().as_str(),
                match action.outcome() {
                    podway_core::ActionOutcomeV2::Next(target) => Some(target.as_str()),
                    podway_core::ActionOutcomeV2::Terminal => None,
                },
                action.skip().is_some(),
            ),
            GraphPlacementV2::Decision(_) => panic!("a conversion never places a decision"),
        })
        .collect();
    assert_eq!(
        placements,
        [
            ("understand", "understand", Some("inspect"), false),
            ("inspect", "inspect", Some("plan"), false),
            ("plan", "plan", Some("implement"), false),
            ("implement", "implement", Some("verify"), false),
            ("verify", "verify", Some("review"), false),
            ("review", "review", Some("finish"), false),
            ("finish", "finish", None, false),
        ],
        "the chain follows stage order and the last stage is the only terminal"
    );
    assert!(
        parsed
            .graph()
            .placements()
            .iter()
            .all(|placement| match placement {
                GraphPlacementV2::Action(action) => action.evidence_from().is_none(),
                GraphPlacementV2::Decision(_) => false,
            }),
        "v1 has no evidence wiring, so a conversion declares none"
    );
    assert!(
        parsed.goal_tracking().is_none(),
        "v1 has no session goal, so a conversion opts into none"
    );
}

/// `skip: {allowed: false}` has no v2 representation, and needs none: an absent skip policy already
/// means "not skippable". `skip: {allowed: true}` carries its v1-effective `reason_required`, which
/// the v1 admission path materializes to `true` when the author omitted it.
#[test]
fn v2aut007_a_disallowed_skip_is_omitted_and_an_allowed_skip_carries_its_effective_reason_flag() {
    let stages = "  - id: alpha\n    title: Alpha\n    skip:\n      allowed: false\n  - id: beta\n    title: Beta\n    skip:\n      allowed: true\n  - id: gamma\n    title: Gamma\n    skip:\n      allowed: true\n      reason_required: false\n";
    let document = converted(&v1_document(None, stages, "[alpha]"));

    assert!(
        !document.document().contains("allowed: false"),
        "v2 forbids `allowed: false`, so a disallowed skip must be omitted entirely:\n{}",
        document.document()
    );
    let parsed = admit_v2(document.document());
    let skips: Vec<(&str, Option<bool>)> = parsed
        .parsed()
        .graph()
        .placements()
        .iter()
        .map(|placement| match placement {
            GraphPlacementV2::Action(action) => (
                action.id().as_str(),
                action.skip().map(|skip| skip.reason_required()),
            ),
            GraphPlacementV2::Decision(_) => unreachable!("actions only"),
        })
        .collect();
    assert_eq!(
        skips,
        [
            ("alpha", None),
            ("beta", Some(true)),
            ("gamma", Some(false)),
        ],
        "an omitted v1 `reason_required` is materialized to true by v1 admission and carried across"
    );
}

/// `any_previous` cannot be authored in v2, whose target list is static. Every node is listed, and
/// the reason — dossier section 9.5's runtime precondition — is written into the document.
#[test]
fn v2aut007_any_previous_expands_to_every_node_with_the_runtime_precondition_comment() {
    let document = converted(&preset_source("sw-dev"));
    let text = document.document();

    assert!(
        text.contains(
            "manual_rework:\n  # Converted from v1 rework.allow_return_to: any_previous; narrow this list.\n  # A v2 target list is static, so cursor-relative previous cannot be authored and every\n  # node is listed; section 9.5 still requires the target to have a valid prior attempt.\n  allowed_targets:\n    - understand\n    - inspect\n    - plan\n    - implement\n    - verify\n    - review\n    - finish\n"
        ),
        "the expansion and its justification are one fixed block above allowed_targets:\n{text}"
    );
}

/// `Only(ids)` is already static, so it maps across verbatim — in v1 order, with no comment,
/// because nothing was over-approximated.
#[test]
fn v2aut007_an_explicit_rework_target_list_is_carried_across_in_v1_order_without_a_comment() {
    let stages = "  - id: alpha\n    title: Alpha\n  - id: beta\n    title: Beta\n  - id: gamma\n    title: Gamma\n";
    let document = converted(&v1_document(None, stages, "[gamma, alpha]"));

    assert!(
        document
            .document()
            .ends_with("manual_rework:\n  allowed_targets:\n    - gamma\n    - alpha\n"),
        "author order survives and no any_previous comment is attached:\n{}",
        document.document()
    );
}

// ---------------------------------------------------------------------------------------------
// Synthesis: P-a, P-b, P-c, and the intent template
// ---------------------------------------------------------------------------------------------

/// **P-a.** A v1 description that fits `purpose` becomes the purpose, and no `description` is
/// written: the author's own sentence is the closest thing v1 has to a stated purpose, and writing
/// it twice would be noise.
#[test]
fn v2aut007_rule_p_a_adopts_a_short_v1_description_as_the_purpose() {
    let document = converted(&v1_document(
        Some("Correct a defect without widening its scope."),
        "  - id: only\n    title: Only stage\n",
        "[only]",
    ));
    let text = document.document();

    assert!(
        text.contains("purpose: Correct a defect without widening its scope.\nnode_definitions:\n"),
        "the adopted purpose is followed directly by node_definitions, so no description was \
         written:\n{text}"
    );
    assert!(
        !text.contains("# Synthesized from the v1 procedure name"),
        "an adopted purpose is the author's own text and carries no review comment:\n{text}"
    );
    assert_eq!(
        admit_v2(text).parsed().description(),
        None,
        "rule P-a writes no v2 description"
    );
}

/// **P-b.** A description too long for `purpose` is kept verbatim as the v2 `description`, and a
/// purpose is synthesized above it. Nothing is cut.
#[test]
fn v2aut007_rule_p_b_keeps_a_long_v1_description_and_synthesizes_the_purpose() {
    let description = "d".repeat(501);
    let document = converted(&v1_document(
        Some(&description),
        "  - id: only\n    title: Only stage\n",
        "[only]",
    ));
    let text = document.document();

    assert!(
        text.contains(&format!(
            "# Synthesized from the v1 procedure name; review and replace.\npurpose: Complete the Fixture workflow.\ndescription: {description}\n"
        )),
        "the synthesized purpose is marked for review and the description survives untouched"
    );
    assert_eq!(
        admit_v2(text).parsed().description(),
        Some(description.as_str()),
        "rule P-b never truncates"
    );
}

/// **P-c.** No description, or one that is only whitespace, means there is nothing to adopt.
#[test]
fn v2aut007_rule_p_c_synthesizes_a_purpose_when_there_is_no_description_to_adopt() {
    for description in [None, Some("\"   \"")] {
        let text = converted(&v1_document(
            description,
            "  - id: only\n    title: Only stage\n",
            "[only]",
        ));
        let text = text.document();
        assert!(
            text.contains(
                "# Synthesized from the v1 procedure name; review and replace.\npurpose: Complete the Fixture workflow.\nnode_definitions:\n"
            ),
            "a synthesized purpose is marked and no description follows it:\n{text}"
        );
        assert_eq!(admit_v2(text).parsed().description(), None);
    }
}

#[test]
fn v2aut007_every_action_intent_is_synthesized_from_its_stage_title_and_marked_for_review() {
    let document = converted(&v1_document(
        None,
        "  - id: alpha\n    title: Inspect the current system\n  - id: beta\n    title: Finish the task\n",
        "[alpha]",
    ));
    let text = document.document();

    for (title, indent) in [
        ("Inspect the current system", "    "),
        ("Finish the task", "    "),
    ] {
        assert!(
            text.contains(&format!(
                "{indent}# Synthesized from the v1 stage title; review and replace.\n{indent}intent: Complete the {title} stage.\n"
            )),
            "every intent is synthesized and marked, at the indentation of the key it annotates:\n{text}"
        );
    }
}

/// The two templates are bounded by construction: a v1 `name` and a v1 stage `title` are each at
/// most 120 characters, and the templates add 23 and 20, so the results are at most 143 and 140 —
/// inside v2's 500-character `purpose` and 300-character `intent`. Asserted at the v1 maximum
/// rather than argued, so a template reworded into something longer fails here.
#[test]
fn v2aut007_the_synthesis_templates_fit_v2_at_the_largest_v1_input() {
    let longest = "n".repeat(120);
    let source = format!(
        "schema: podway.procedure/v1\nid: fixture\nversion: \"1\"\nname: {longest}\nstages:\n  - id: only\n    title: {longest}\nrework:\n  allow_return_to: [only]\n"
    );
    let parsed = admit_v2(converted(&source).document());
    let parsed = parsed.parsed();

    assert_eq!(parsed.purpose().chars().count(), 143);
    let ParsedNodeDefinition::Action(action) = &parsed.node_definitions()[0] else {
        panic!("a conversion emits action definitions only");
    };
    assert_eq!(action.intent().chars().count(), 140);
}

// ---------------------------------------------------------------------------------------------
// Item mapping
// ---------------------------------------------------------------------------------------------

/// Every v1-effective item field is written explicitly, including the ones v1 defaulted. This is
/// the assertion that makes the conversion meaning-preserving rather than shape-preserving: v1 and
/// v2 disagree about `max_length` (8,000 against 4,000), `max_items` (100 against 50), and
/// `max_item_length` agrees only by coincidence, so an omitted field would silently retune the item.
#[test]
fn v2aut007_v1_effective_item_defaults_are_written_explicitly_because_the_v2_defaults_differ() {
    let items = "      - id: prose\n        type: text\n        prompt: Write it.\n        required: true\n      - id: entries\n        type: list\n        prompt: List them.\n        required: false\n";
    let document = converted(&v1_with_item(items));

    assert!(
        document.document().contains(
            "      - id: prose\n        type: text\n        prompt: Write it.\n        required: true\n        min_length: 0\n        max_length: 8000\n        multiline: true\n"
        ),
        "an omitted v1 text max_length is the v1 default 8000, not the v2 default 4000:\n{}",
        document.document()
    );
    assert!(
        document.document().contains(
            "      - id: entries\n        type: list\n        prompt: List them.\n        required: false\n        min_items: 0\n        max_items: 100\n        max_item_length: 500\n        unique: true\n"
        ),
        "an omitted v1 list max_items is the v1 default 100, not the v2 default 50:\n{}",
        document.document()
    );
}

#[test]
fn v2aut007_every_item_type_maps_field_for_field() {
    let items = concat!(
        "      - id: agreed\n        type: confirm\n        prompt: Agreed.\n        help: Say yes.\n        required: true\n",
        "      - id: note\n        type: text\n        prompt: Note.\n        required: false\n        min_length: 2\n        max_length: 40\n        multiline: false\n",
        "      - id: level\n        type: choice\n        prompt: Level.\n        required: true\n        choices: [low, high]\n",
        "      - id: count\n        type: integer\n        prompt: Count.\n        required: false\n        minimum: -5\n        maximum: 9\n",
        "      - id: steps\n        type: list\n        prompt: Steps.\n        required: true\n        min_items: 1\n        max_items: 3\n        max_item_length: 20\n        unique: false\n",
        "      - id: report\n        type: artifact\n        prompt: Report.\n        required: false\n        allowed_media_types: [text/plain]\n",
    );
    let document = converted(&v1_with_item(items));

    assert!(
        document.document().contains(concat!(
            "    items:\n",
            "      - id: agreed\n        type: confirm\n        prompt: Agreed.\n        help: Say yes.\n        required: true\n",
            "      - id: note\n        type: text\n        prompt: Note.\n        required: false\n        min_length: 2\n        max_length: 40\n        multiline: false\n",
            "      - id: level\n        type: choice\n        prompt: Level.\n        required: true\n        choices:\n          - low\n          - high\n",
            "      - id: count\n        type: integer\n        prompt: Count.\n        required: false\n        minimum: -5\n        maximum: 9\n",
            "      - id: steps\n        type: list\n        prompt: Steps.\n        required: true\n        min_items: 1\n        max_items: 3\n        max_item_length: 20\n        unique: false\n",
            "      - id: report\n        type: artifact\n        prompt: Report.\n        required: false\n        allowed_media_types:\n          - text/plain\n",
        )),
        "item order and every field survive:\n{}",
        document.document()
    );
}

/// A v1 stage with no instructions and no items produces no empty collections: v2 gives each
/// optional collection `minItems: 1` and rejects an explicitly empty one.
#[test]
fn v2aut007_empty_v1_collections_are_omitted_rather_than_written_as_empty_ones() {
    let document = converted(&v1_document(
        None,
        "  - id: only\n    title: Only stage\n    instructions: []\n    items: []\n",
        "[only]",
    ));
    let text = document.document();

    assert!(!text.contains("instructions"), "{text}");
    assert!(!text.contains("items"), "{text}");
    assert!(
        !text.contains("[]"),
        "canonical authoring form has no empty-collection literal to write:\n{text}"
    );
}

// ---------------------------------------------------------------------------------------------
// Overflow: the classes that are reachable, and the ones that are not
// ---------------------------------------------------------------------------------------------

// The complete enumeration, verified against `crates/podway-config/src/lib.rs` (the v1 bounds),
// `crates/podway-core/src/procedure_v2/{items,definitions,graph}.rs`, and
// `crates/podway-config/src/procedure_v2_parse.rs` (the v2 bounds).
//
// Reachable from a legal v1 document, each with a test below:
//   O1  procedure description  v1 4,000 -> v2 1,000 (only past rule P-b's 1,000-char fallback)
//   O2  instructions per stage v1    32 -> v2    16
//   O3  instruction characters v1 2,000 -> v2 1,000
//   O4  items per stage        v1   128 -> v2    64
//   O5  item prompt            v1   500 -> v2   300
//   O6  item help              v1 4,000 -> v2 1,000
//   O8  text max_length        v1 65,536 -> v2 16,384
//   O9  choice values          v1    64 -> v2    32
//   O11 list max_items         v1 1,000 -> v2   100
//   O12 list max_item_length   v1 4,000 -> v2 1,000
//   O13 emitted document / canonical projection over SOURCE_PROJECTION_MAX_CHARACTERS
//
// Reachable only jointly, never alone — reported anyway, because both values need editing:
//   O7  text min_length. v2 requires `min_length <= max_length <= 16,384`, so a `min_length` past
//       16,384 drags a `max_length` past 16,384 with it, and O8 always fires beside it.
//   O10 list min_items. Same shape: v2 requires `min_items <= max_items <= 100`.
//
// Not reachable at all, because v1 is at least as strict as v2:
//   procedure id/version/name, stage id/title, item id (identical bounds); stage count (1..64 on
//   both sides, and the graph and definition maps are both capped at 64); rework target count
//   (1..64 on both sides); integer minimum/maximum (`i64` on both sides); artifact
//   allowed_media_types (0..64 against 0..64, with the same media-type grammar).

#[test]
fn v2aut007_o1_a_v1_description_too_long_for_the_v2_description_bound_is_refused() {
    let description = "d".repeat(1_001);
    let source = v1_document(
        Some(&description),
        "  - id: only\n    title: Only stage\n",
        "[only]",
    );
    assert_eq!(
        overflow_report(&source),
        vec![(
            "AUTHORING_SCHEMA_INVALID",
            "description".to_owned(),
            "The v1 procedure description is 1001 characters, over the 1000-character Procedure v2 \
             description bound."
                .to_owned(),
        )]
    );

    // The bound pin: one character less is rule P-b, not an overflow.
    let inside = v1_document(
        Some(&"d".repeat(1_000)),
        "  - id: only\n    title: Only stage\n",
        "[only]",
    );
    assert_eq!(
        admit_v2(converted(&inside).document())
            .parsed()
            .description()
            .map(str::len),
        Some(1_000)
    );
}

#[test]
fn v2aut007_o2_and_o3_bound_the_instruction_list_and_each_instruction() {
    let instructions = |count: usize, length: usize| {
        let entries = (0..count)
            .map(|_| format!("      - {}\n", "i".repeat(length)))
            .collect::<String>();
        v1_document(
            None,
            &format!("  - id: only\n    title: Only stage\n    instructions:\n{entries}"),
            "[only]",
        )
    };

    assert_eq!(
        overflow_codes(&instructions(17, 4)),
        [(
            "AUTHORING_SCHEMA_INVALID",
            "stages[only].instructions".to_owned()
        )]
    );
    assert!(convert(&instructions(16, 4)).is_ok(), "16 is the v2 bound");

    assert_eq!(
        overflow_codes(&instructions(1, 1_001)),
        [(
            "AUTHORING_SCHEMA_INVALID",
            "stages[only].instructions[0]".to_owned()
        )]
    );
    assert!(
        convert(&instructions(1, 1_000)).is_ok(),
        "1,000 is the v2 bound"
    );
}

#[test]
fn v2aut007_o4_bounds_the_item_list() {
    let items = |count: usize| {
        let entries = (0..count)
            .map(|index| {
                format!(
                    "      - id: item-{index}\n        type: confirm\n        prompt: Confirm.\n        required: true\n"
                )
            })
            .collect::<String>();
        v1_with_item(&entries)
    };

    assert_eq!(
        overflow_codes(&items(65)),
        [("AUTHORING_SCHEMA_INVALID", "stages[only].items".to_owned())]
    );
    assert!(convert(&items(64)).is_ok(), "64 is the v2 bound");
}

#[test]
fn v2aut007_o5_and_o6_bound_the_item_prompt_and_help() {
    let item = |prompt: usize, help: usize| {
        v1_with_item(&format!(
            "      - id: field\n        type: confirm\n        prompt: {}\n        help: {}\n        required: true\n",
            "p".repeat(prompt),
            "h".repeat(help),
        ))
    };

    assert_eq!(
        overflow_codes(&item(301, 4)),
        [(
            "AUTHORING_SCHEMA_INVALID",
            "stages[only].items[field].prompt".to_owned()
        )]
    );
    assert!(convert(&item(300, 4)).is_ok(), "300 is the v2 bound");

    assert_eq!(
        overflow_codes(&item(4, 1_001)),
        [(
            "AUTHORING_SCHEMA_INVALID",
            "stages[only].items[field].help".to_owned()
        )]
    );
    assert!(convert(&item(4, 1_000)).is_ok(), "1,000 is the v2 bound");
}

/// O8 alone, then O7 arriving beside it — the pair that cannot be separated.
#[test]
fn v2aut007_o7_and_o8_bound_both_text_lengths_and_o7_never_fires_alone() {
    let text = |min: u32, max: u32| {
        v1_with_item(&format!(
            "      - id: field\n        type: text\n        prompt: Write.\n        required: true\n        min_length: {min}\n        max_length: {max}\n"
        ))
    };

    assert!(convert(&text(0, 16_384)).is_ok(), "16,384 is the v2 bound");
    assert_eq!(
        overflow_codes(&text(0, 16_385)),
        [(
            "AUTHORING_SCHEMA_INVALID",
            "stages[only].items[field].max_length".to_owned()
        )],
        "an oversized maximum is reported on its own"
    );
    assert_eq!(
        overflow_codes(&text(16_385, 16_385)),
        [
            (
                "AUTHORING_SCHEMA_INVALID",
                "stages[only].items[field].min_length".to_owned()
            ),
            (
                "AUTHORING_SCHEMA_INVALID",
                "stages[only].items[field].max_length".to_owned()
            ),
        ],
        "v2 requires min_length <= max_length <= 16,384, so O7 can only ever arrive with O8; both \
         are reported because both have to be edited, in v1 source order"
    );
}

#[test]
fn v2aut007_o9_bounds_the_choice_values() {
    let choices = |count: usize| {
        let values = (0..count)
            .map(|index| format!("          - c{index}\n"))
            .collect::<String>();
        v1_with_item(&format!(
            "      - id: field\n        type: choice\n        prompt: Pick.\n        required: true\n        choices:\n{values}"
        ))
    };

    assert_eq!(
        overflow_codes(&choices(33)),
        [(
            "AUTHORING_SCHEMA_INVALID",
            "stages[only].items[field].choices".to_owned()
        )]
    );
    assert!(convert(&choices(32)).is_ok(), "32 is the v2 bound");
}

/// O11 and O12 alone, then O10 arriving beside O11 — the second inseparable pair.
#[test]
fn v2aut007_o10_o11_and_o12_bound_the_list_constraints() {
    let list = |min: u16, max: u16, length: u16| {
        v1_with_item(&format!(
            "      - id: field\n        type: list\n        prompt: List.\n        required: true\n        min_items: {min}\n        max_items: {max}\n        max_item_length: {length}\n"
        ))
    };

    assert!(convert(&list(0, 100, 1_000)).is_ok(), "the v2 bounds");
    assert_eq!(
        overflow_codes(&list(0, 101, 500)),
        [(
            "AUTHORING_SCHEMA_INVALID",
            "stages[only].items[field].max_items".to_owned()
        )]
    );
    assert_eq!(
        overflow_codes(&list(0, 100, 1_001)),
        [(
            "AUTHORING_SCHEMA_INVALID",
            "stages[only].items[field].max_item_length".to_owned()
        )]
    );
    assert_eq!(
        overflow_codes(&list(101, 101, 500)),
        [
            (
                "AUTHORING_SCHEMA_INVALID",
                "stages[only].items[field].min_items".to_owned()
            ),
            (
                "AUTHORING_SCHEMA_INVALID",
                "stages[only].items[field].max_items".to_owned()
            ),
        ],
        "v2 requires min_items <= max_items <= 100, so O10 can only ever arrive with O11"
    );
}

/// O13. A v1 document well inside its own one-megabyte admission limit can still project past the
/// 131,072-character source-projection budget, because the v2 authoring text materializes defaults
/// the v1 text left implicit.
#[test]
fn v2aut007_o13_a_candidate_over_the_source_projection_budget_is_refused() {
    let instruction = "i".repeat(1_000);
    let stages = (0..20)
        .map(|index| {
            let entries = (0..16)
                .map(|_| format!("      - {instruction}\n"))
                .collect::<String>();
            format!("  - id: stage-{index}\n    title: Stage {index}\n    instructions:\n{entries}")
        })
        .collect::<String>();
    let source = v1_document(None, &stages, "[stage-0]");
    assert!(
        source.len() < podway_config::MAX_PROCEDURE_DOCUMENT_BYTES_V1,
        "the v1 source itself must be admissible, or the test proves nothing about conversion"
    );

    let report = overflow_report(&source);
    assert_eq!(report.len(), 1, "{report:#?}");
    assert_eq!(report[0].0, "SOURCE_PROJECTION_BUDGET_EXCEEDED");
    assert_eq!(report[0].1, "$", "a budget is a document-level finding");
}

/// Convert walks the whole model rather than stopping at the first refusal, so one pass reports
/// every edit the author has to make.
#[test]
fn v2aut007_every_overflow_in_a_document_is_reported_in_one_pass() {
    let stages = format!(
        "  - id: alpha\n    title: Alpha\n    items:\n      - id: one\n        type: confirm\n        prompt: {}\n        required: true\n  - id: beta\n    title: Beta\n    items:\n      - id: two\n        type: list\n        prompt: List.\n        required: true\n        max_items: 500\n        max_item_length: 2000\n",
        "p".repeat(301)
    );
    let source = v1_document(None, &stages, "[alpha]");

    assert_eq!(
        overflow_codes(&source),
        [
            (
                "AUTHORING_SCHEMA_INVALID",
                "stages[alpha].items[one].prompt".to_owned()
            ),
            (
                "AUTHORING_SCHEMA_INVALID",
                "stages[beta].items[two].max_items".to_owned()
            ),
            (
                "AUTHORING_SCHEMA_INVALID",
                "stages[beta].items[two].max_item_length".to_owned()
            ),
        ],
        "three findings from two stages, in the shared report order — which is v1 source order, \
         because every conversion finding is located in the v1 document"
    );
}

/// Every overflow points at the v1 source position of the value it names, not at the document
/// start: the file the author still has is the v1 file.
#[test]
fn v2aut007_an_overflow_locates_the_offending_value_in_the_v1_source() {
    let source = v1_with_item(&format!(
        "      - id: field\n        type: list\n        prompt: List.\n        required: true\n        max_items: {}\n",
        500
    ));
    let diagnostics = convert(&source).expect_err("the fixture must overflow");
    let diagnostic = &diagnostics[0];

    let line = usize::try_from(diagnostic.location().line()).expect("a source line fits");
    assert_eq!(
        source.lines().nth(line - 1).map(str::trim),
        Some("max_items: 500"),
        "the reported line is the line the offending value is written on"
    );
    assert_eq!(diagnostic.source_path(), "workflow.yaml");
}

// ---------------------------------------------------------------------------------------------
// Bound pins: the converter's constants against the constructors that enforce them
// ---------------------------------------------------------------------------------------------

/// The converter pre-checks the v2 bounds with its own constants, because the real ones are private
/// to their constructors. This test makes the duplication safe in both directions at once: for each
/// bound, a v1 document *at* the bound must convert (so the constant is not stricter than v2) and a
/// v1 document one past it must be refused by the converter's own scan rather than by the
/// constructor behind it (so the constant is not looser than v2).
///
/// The "rather than by the constructor" half is what the `field` assertion buys: a scan-produced
/// diagnostic names a v1 path like `stages[only].items[field].max_length`, while the defensive
/// fallback that catches a constructor rejection names a v2 shape or the document root. If a v2
/// bound were tightened without this file being updated, the field would change and this fails.
#[test]
fn v2aut007_every_pre_checked_bound_matches_the_v2_constructor_that_enforces_it() {
    let text_item = |field: &str, value: String| {
        v1_with_item(&format!(
            "      - id: field\n        type: {field}\n        prompt: Prompt.\n        required: true\n{value}"
        ))
    };
    let cases: Vec<(&str, String, String, String)> = vec![
        (
            "purpose/description",
            "description".to_owned(),
            v1_document(
                Some(&"d".repeat(1_000)),
                "  - id: only\n    title: Only stage\n",
                "[only]",
            ),
            v1_document(
                Some(&"d".repeat(1_001)),
                "  - id: only\n    title: Only stage\n",
                "[only]",
            ),
        ),
        (
            "text max_length",
            "stages[only].items[field].max_length".to_owned(),
            text_item("text", "        max_length: 16384\n".to_owned()),
            text_item("text", "        max_length: 16385\n".to_owned()),
        ),
        (
            "list max_items",
            "stages[only].items[field].max_items".to_owned(),
            text_item("list", "        max_items: 100\n".to_owned()),
            text_item("list", "        max_items: 101\n".to_owned()),
        ),
        (
            "list max_item_length",
            "stages[only].items[field].max_item_length".to_owned(),
            text_item("list", "        max_item_length: 1000\n".to_owned()),
            text_item("list", "        max_item_length: 1001\n".to_owned()),
        ),
    ];

    for (label, field, at_bound, past_bound) in cases {
        assert!(
            convert(&at_bound).is_ok(),
            "{label}: the v2 bound itself must convert"
        );
        assert_eq!(
            overflow_codes(&past_bound),
            [("AUTHORING_SCHEMA_INVALID", field)],
            "{label}: one past the bound must be caught by the converter's own scan"
        );
    }
}

// ---------------------------------------------------------------------------------------------
// Purity
// ---------------------------------------------------------------------------------------------

/// Converting does not disturb the v1 model it read, so a caller can convert and then keep using
/// the validated v1 procedure — which is what the CLI does when it reports `source_digest`.
#[test]
fn v2aut007_conversion_leaves_the_validated_v1_procedure_untouched() {
    let source = preset_source("sw-dev");
    let validated = admit_v1(&source);
    let before = validated.clone();

    let context = AuthoringContext::new("sw-dev.yaml", &source, ProcedureDocumentFormat::Yaml);
    convert_procedure_v1_to_v2(&validated, &context).expect("the preset converts");

    assert_eq!(validated, before);
}
