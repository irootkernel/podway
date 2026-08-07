//! V2MOD-008 "Lock v1 configuration compatibility" — dossier §19.2 acceptance gate: "All existing
//! v1 fixtures and released canonical results remain byte-for-byte stable; run the complete v1
//! config regression suite."
//!
//! This file is the lock, not a feature. It carries no `src/` change: every assertion below
//! re-pins a v1 golden value that already existed before the Procedure v2 pipeline landed, as an
//! *independent* literal duplicated from its original source test — so a canonicalization,
//! digest, limit, or dispatch regression must break two tests in two files to go unnoticed — and
//! it proves the config-owned half of V2ACC-069 ("v1 commands are not silently reinterpreted
//! under v2"): every production call site that admits v1 input calls `parse_procedure_v1`
//! directly, never the schema-dispatching `parse_procedure_document` / `parse_procedure_yaml`.
//!
//! If any assertion here fails, that is drift introduced by the v2 work, not a stale literal.
//! Do not edit a literal to make it pass — report the mismatch (both values) instead.

use podway_config::{
    ConfigError, DEFAULT_WORKSPACE_CONFIG_YAML_V1, MAX_PROCEDURE_DOCUMENT_BYTES_V1,
    MAX_PROCEDURE_DOCUMENT_DEPTH_V1, MAX_PROCEDURE_DOCUMENT_NODES_V1,
    MAX_WORKSPACE_CONFIG_BYTES_V1, MAX_WORKSPACE_CONFIG_DEPTH_V1, MAX_WORKSPACE_CONFIG_NODES_V1,
    ParsedProcedure, ProcedureFormatV1, ProcedureWarningV1, parse_procedure_document,
    parse_procedure_v1,
};
use podway_core::ProcedureWarningCodeV1;
use serde_json::{Value, json};

/// The shared golden v1 document reused by items 1, 2, and 3 below: it is exactly
/// `int_procedure_v1.rs`'s `base_document()` (same id/stage/item shape, same field order
/// irrelevant since canonicalization is order-independent).
fn golden_v1_document() -> Value {
    json!({
        "schema": "podway.procedure/v1",
        "id": "release",
        "version": "1",
        "name": "Release",
        "stages": [{
            "id": "prepare",
            "title": "Prepare",
            "items": [{
                "id": "approval",
                "type": "confirm",
                "prompt": "Approved",
                "required": true
            }]
        }],
        "rework": { "allow_return_to": ["prepare"] }
    })
}

/// The YAML form of [`golden_v1_document`] — exactly `int_procedure_v1.rs`'s
/// `base_yaml_document()`.
const GOLDEN_V1_YAML: &str = concat!(
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

/// Golden canonical bytes for [`golden_v1_document`], copied verbatim from
/// `int_procedure_v1.rs::canonical_json_and_digest_match_golden_fixture`.
const GOLDEN_V1_CANONICAL_JSON: &[u8] = br#"{"id":"release","name":"Release","rework":{"allow_return_to":["prepare"]},"schema":"podway.procedure/v1","stages":[{"id":"prepare","instructions":[],"items":[{"id":"approval","prompt":"Approved","required":true,"type":"confirm"}],"title":"Prepare"}],"version":"1"}"#;
/// Golden digest for [`golden_v1_document`], copied verbatim from the same source test.
const GOLDEN_V1_DIGEST: &str =
    "sha256:2d2e9f9453b36610949989fadbae5b3665778701aae59727fe4fa1330b5e0c5a";

// --- Item 1: golden byte/digest re-pins ---------------------------------------------------

/// Re-pins `int_procedure_v1.rs::canonical_json_and_digest_match_golden_fixture` as an
/// independent lock: same input document, same expected canonical bytes and digest, duplicated
/// here on purpose.
#[test]
fn golden_v1_canonical_digest_matches_procedure_v1_suite_pin() {
    let procedure =
        parse_procedure_v1(golden_v1_document().to_string(), ProcedureFormatV1::Json).unwrap();

    assert_eq!(
        procedure.canonical_json().as_bytes(),
        GOLDEN_V1_CANONICAL_JSON
    );
    assert_eq!(procedure.digest().as_str(), GOLDEN_V1_DIGEST);
}

/// Re-pins the normalized-skip canonical-JSON golden from
/// `crates/podway-config/src/validation.rs`'s unit test
/// `config_production_normalizes_skip_policies_and_matches_core_verification`. That unit test
/// calls `ValidatedProcedureV1::new` directly, which is `pub(crate)` and unreachable from an
/// integration test, so this reconstructs the identical semantic input — one stage named
/// `"stage"`, `skip: { allowed: true }` with `reason_required` omitted (so
/// `apply_documented_defaults` fills it to `true`, exactly as the unit test's
/// `definition_with_skip` input does), `rework: any_previous` — through the public
/// `parse_procedure_v1` entry point instead. The canonical-JSON literal below is copied verbatim
/// from that unit test; the digest is not pinned anywhere else, so it is computed once from those
/// exact bytes and pinned here for the first time.
#[test]
fn golden_v1_normalized_skip_canonical_json_matches_validation_suite_pin() {
    let document = json!({
        "schema": "podway.procedure/v1",
        "id": "codec",
        "version": "1",
        "name": "Codec",
        "stages": [{
            "id": "stage",
            "title": "Stage",
            "skip": { "allowed": true }
        }],
        "rework": { "allow_return_to": "any_previous" }
    });
    let procedure = parse_procedure_v1(document.to_string(), ProcedureFormatV1::Json).unwrap();

    assert_eq!(
        procedure.canonical_json().as_str(),
        r#"{"id":"codec","name":"Codec","rework":{"allow_return_to":"any_previous"},"schema":"podway.procedure/v1","stages":[{"id":"stage","instructions":[],"items":[],"skip":{"allowed":true,"reason_required":true},"title":"Stage"}],"version":"1"}"#
    );
    assert_eq!(
        procedure.digest().as_str(),
        "sha256:91fde0a7c5b91d582055fde12c3d7e79e55152b547628e1678f107ab0d7e95d7"
    );
}

/// Re-pins `DEFAULT_WORKSPACE_CONFIG_YAML_V1`'s exact bytes, independently of
/// `int_phase4_workspace_config.rs::default_workspace_config_bytes_are_explicit_canonical_and_replayable`
/// (same literal, duplicated on purpose). Full text is pinned rather than a digest because the
/// constant is short.
#[test]
fn golden_default_workspace_config_yaml_v1_bytes_are_pinned() {
    assert_eq!(
        DEFAULT_WORKSPACE_CONFIG_YAML_V1,
        b"schema: podway.workspace/v1\nprocedure_paths:\n  - .podway/procedures\ndefault_preset: sw-dev\njob_queue:\n  max_pending: 256\nui:\n  show_stage_in_prompt: false\n",
    );
}

/// Every v1 resource bound, pinned against literal numbers. Dossier §5.1: "v1 sessions keep every
/// v1 bound unchanged" — the v2 pipeline landing alongside these must not have moved any of them,
/// even by one.
#[test]
fn golden_v1_resource_bounds_are_unchanged() {
    assert_eq!(MAX_PROCEDURE_DOCUMENT_BYTES_V1, 1_048_576);
    assert_eq!(MAX_PROCEDURE_DOCUMENT_DEPTH_V1, 64);
    assert_eq!(MAX_PROCEDURE_DOCUMENT_NODES_V1, 100_000);
    assert_eq!(MAX_WORKSPACE_CONFIG_BYTES_V1, 65_536);
    assert_eq!(MAX_WORKSPACE_CONFIG_DEPTH_V1, 16);
    assert_eq!(MAX_WORKSPACE_CONFIG_NODES_V1, 1_024);
}

// --- Item 2: dispatch isolation -------------------------------------------------------------
//
// Structural argument this file encodes in tests, not just prose: every production call site
// that admits a v1 procedure document calls `parse_procedure_v1` directly. Verified by grep
// across `crates/*/src` while writing this file, and re-run at V2AUT-007:
//   - crates/podway-cli/src/command.rs:1539     (fn execute_start_dry_run — workspace-path
//     branch of `session.start`)
//   - crates/podway-cli/src/command.rs:3002     (fn execute_procedure — `procedure.validate` /
//     `procedure.show`)
//   - crates/podway-cli/src/command.rs:3636     (fn execute_procedure_convert —
//     `procedure.convert`, V2AUT-007. Convert's *input* is a v1 document, so it is a v1 admission
//     call site like any other: it refuses a declared-v2 document up front via
//     `sniff_procedure_schema` and then hands everything else to the direct v1 parser, which is
//     what makes its failure for a malformed v1 file byte-identical to `procedure validate`'s.)
//   - crates/podway-daemon/src/execution.rs:217 (fn workspace_procedure_snapshot_from_bytes_v1)
//   - crates/podway-presets/src/lib.rs:48        (impl EmbeddedPreset::validate)
//   - crates/podway-config/src/parser.rs         (the `PROCEDURE_SCHEMA_V1` arm inside
//     `parse_procedure_document` itself — the dispatcher delegates to the direct parser, so both
//     paths admit v1 through the same function)
// A further textual match, crates/podway-daemon/src/production.rs:3107, sits inside that file's
// own `#[cfg(test)] mod tests` (opened at line 2777) and is not a production call site.
//
// Three production call sites of the schema-dispatching `parse_procedure_document` exist, and
// none of them admits v1:
//   - crates/podway-config/src/procedure_v2_format.rs:238 (fn admit_procedure_v2, V2AUT-001 —
//     also the path `procedure format` and `procedure check` reach it by)
//   - crates/podway-cli/src/command.rs:3357               (fn execute_procedure_lint, V2AUT-004)
//   - crates/podway-cli/src/command.rs:3576               (fn execute_procedure_scaffold,
//     V2AUT-006 — its input is a checked-in v2 template, never a file)
// The first two sniff the decoded `schema` field (`sniff_procedure_schema`) before dispatching and
// refuse a declared-v1 document — malformed or not — as a wrong-schema command failure, so a v1
// document reaching `podway procedure format`, `lint`, or `check` is never reinterpreted as a v2
// authoring finding; the dispatcher's `V1` arm remains only as the total-match backstop behind
// that sniff. No other module under `crates/*/src` calls `parse_procedure_v1`,
// `parse_procedure_document`, or `parse_procedure_yaml` (the remaining matches are `parser.rs`'s
// own definitions and `parse_procedure_yaml`'s one-line delegation).
//
// Because no runtime v1 admission path routes through the schema-dispatching function, no v1
// runtime command can be silently reinterpreted as v2 — that's V2ACC-069's config-level substance.
// The two tests below close the loop the call-site survey opens: the direct and dispatched parses
// agree for v1 input, and the direct v1 parser itself refuses a v2-declared schema.

/// `ValidatedProcedureV1` derives `Eq`/`PartialEq` over all four of its fields — `definition`,
/// `canonical_json`, `digest`, `warnings` (`crates/podway-config/src/validation.rs`) — and so
/// does the `ParsedProcedure` enum that wraps it. A direct `assert_eq!` between the
/// `parse_procedure_document` dispatch result and the direct `parse_procedure_v1` result is
/// therefore already a complete comparison of definition, canonical bytes, digest, and warnings
/// together; no piecewise field-by-field comparison is needed.
#[test]
fn dispatch_v1_yaml_and_json_equal_direct_parse_v1() {
    let direct_yaml = parse_procedure_v1(GOLDEN_V1_YAML, ProcedureFormatV1::Yaml)
        .expect("representative v1 YAML document parses directly");
    assert_eq!(
        parse_procedure_document(GOLDEN_V1_YAML.as_bytes(), ProcedureFormatV1::Yaml),
        Ok(ParsedProcedure::V1(direct_yaml)),
    );

    let json_bytes = golden_v1_document().to_string();
    let direct_json = parse_procedure_v1(&json_bytes, ProcedureFormatV1::Json)
        .expect("representative v1 JSON document parses directly");
    assert_eq!(
        parse_procedure_document(json_bytes.as_bytes(), ProcedureFormatV1::Json),
        Ok(ParsedProcedure::V1(direct_json)),
    );
}

/// A document whose `schema` says v2 but is otherwise v1-shaped (has `stages`/`rework`, not
/// `purpose`/`node_definitions`/`graph`) fails `parse_procedure_v1` with exactly
/// `InvalidSchema { expected: "podway.procedure/v1", actual: "podway.procedure/v2" }` and nothing
/// else — the v1 entry point rejects on the declared schema; it does not attempt to interpret v2
/// content as v1.
///
/// (A document that is genuinely v2-shaped instead fails `ProcedureDefinitionV1` deserialization
/// with `InvalidDocument` — missing `stages`/`rework`, unknown `purpose` under
/// `#[serde(deny_unknown_fields)]` — before the schema check ever runs. That is still fail-closed,
/// just a different `ConfigError` variant, so it is a different property and not what this test
/// pins.)
#[test]
fn parse_procedure_v1_rejects_v2_schema_with_invalid_schema_and_nothing_else() {
    let mut document = golden_v1_document();
    document["schema"] = json!("podway.procedure/v2");

    assert_eq!(
        parse_procedure_v1(document.to_string(), ProcedureFormatV1::Json),
        Err(ConfigError::InvalidSchema {
            expected: "podway.procedure/v1",
            actual: "podway.procedure/v2".to_owned(),
        })
    );
}

// A v1-schema document reaching the v2 *validate* path (`validate_procedure_v2`) is impossible by
// construction, not merely by runtime check, so no test exists for it: `validate_procedure_v2`
// takes a `ParsedProcedureV2`, whose fields are private to `podway-config` and whose only
// constructors are the crate-private `parse_procedure_v2_yaml` / `parse_procedure_v2_json`
// (`crates/podway-config/src/procedure_v2_parse.rs`). `parse_procedure_document` calls those only
// after the document's `schema` has already matched `podway_core::PROCEDURE_SCHEMA_V2` exactly.
// There is no code path that produces a `ParsedProcedureV2` value from v1 content, so there is
// nothing to assert at runtime.

// --- Item 3: compatibility fixture consumption ------------------------------------------------

const V1_BOUNDARIES_FIXTURE: &str =
    include_str!("../../../tests/fixtures/v2/compatibility/v1-boundaries.json");

/// Consumes the `released-v1-fixtures` case of `tests/fixtures/v2/compatibility/v1-boundaries.json`
/// — the one case whose `expected` value is literally `"byte-for-byte-stable"`, matching the
/// dossier §19.2 acceptance gate this whole file locks. Reads the fixture at test time (not just
/// an assumption baked into this file), so a rename or a changed `expected` value fails loudly
/// instead of silently going stale, then executes the case's config-ownable meaning: the item-1
/// golden v1 document still round-trips to byte-for-byte identical canonical bytes and digest.
///
/// The fixture's other cases are not config's to execute; each is annotated here with its real
/// owning task per `quality/v2-compatibility-matrix-v1.json` rather than faked:
///   - `"v1-storage-migration"` -> V2COMP-002 / V2PLT-001 (v1 session-store migration).
///   - `"v1-command-dispatch"` -> V2COMP-004 / V2MOD-008. The fixture's full case ("dispatch
///     every v1 command against migrated state") needs migrated storage (V2PLT-001) plus live
///     CLI/daemon command dispatch; this file's `dispatch_v1_*` tests above are the
///     config-crate-ownable slice of that same V2ACC-069 substance (the call-site argument), not
///     the full end-to-end case.
///   - `"v1-reopen"` -> V2COMP-005 / V2COMP-SURFACE-007, both filed under V2REL-001 but scoped to
///     session-command reopen/reactivation handling in `podway-core` / `podway-daemon` — outside
///     this task's config-crate file scope (`crates/podway-config/tests/**`,
///     `crates/podway-presets/tests/**` only).
///   - `"current-task-retention"` -> V2COMP-007 / V2PLT-005 (reset/recovery retention boundary).
///   - `"existing-route-v2-result-family"` -> V2COMP-SURFACE-001 / V2REL-001 (result schemas).
///   - `"new-route-v1-result-family"` -> V2COMP-SURFACE-002 / V2REL-001.
///   - `"v2-never-extends-v1-result-family"` -> V2COMP-SURFACE-003 / V2REL-001.
///   - `"manifest-digest-capability-discovery"` -> V2COMP-SURFACE-006 / V2REL-001 (peer manifest
///     capability discovery).
#[test]
fn v1_boundaries_fixture_released_v1_fixtures_case_is_byte_for_byte_stable() {
    let fixture: Value =
        serde_json::from_str(V1_BOUNDARIES_FIXTURE).expect("fixture is valid JSON");
    let cases = fixture["cases"]
        .as_array()
        .expect("fixture has a cases array");

    let released = cases
        .iter()
        .find(|case| case["id"].as_str() == Some("released-v1-fixtures"))
        .expect("v1-boundaries.json must define a released-v1-fixtures case");
    assert_eq!(released["expected"].as_str(), Some("byte-for-byte-stable"));

    // Every case this doc comment annotates must still be present, so a rename does not silently
    // orphan an annotation above.
    for expected_id in [
        "released-v1-fixtures",
        "v1-storage-migration",
        "v1-command-dispatch",
        "v1-reopen",
        "current-task-retention",
        "existing-route-v2-result-family",
        "new-route-v1-result-family",
        "v2-never-extends-v1-result-family",
        "manifest-digest-capability-discovery",
    ] {
        assert!(
            cases
                .iter()
                .any(|case| case["id"].as_str() == Some(expected_id)),
            "annotated case {expected_id} is missing from v1-boundaries.json"
        );
    }

    let procedure =
        parse_procedure_v1(golden_v1_document().to_string(), ProcedureFormatV1::Json).unwrap();
    assert_eq!(
        procedure.canonical_json().as_bytes(),
        GOLDEN_V1_CANONICAL_JSON
    );
    assert_eq!(procedure.digest().as_str(), GOLDEN_V1_DIGEST);
}

// --- Item 4: v1 warning behavior unchanged ----------------------------------------------------

/// Re-pins v1 semantic-warning behavior against a minimal deterministic fixture, following the
/// precedent set by `int_procedure_v1.rs`'s
/// `semantic_validation_rejects_boundaries_and_emits_every_warning_category` and
/// `near_hard_limit_warnings_are_inclusive_and_scoped`, which pin larger warning sets the same
/// way. Swapping the golden document's `rework.allow_return_to` from `["prepare"]` to
/// `"any_previous"` trips exactly one rule: `AnyPreviousReturnPolicy`.
#[test]
fn golden_v1_any_previous_warning_is_pinned() {
    let mut document = golden_v1_document();
    document["rework"] = json!({ "allow_return_to": "any_previous" });
    let procedure = parse_procedure_v1(document.to_string(), ProcedureFormatV1::Json).unwrap();

    assert_eq!(
        procedure.warnings(),
        &[ProcedureWarningV1 {
            code: ProcedureWarningCodeV1::AnyPreviousReturnPolicy,
            stage_id: None,
            item_id: None,
        }]
    );
}
