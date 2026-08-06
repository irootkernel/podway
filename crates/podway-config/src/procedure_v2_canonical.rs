//! Procedure v2 canonicalization: the model-derived canonical document, its Canonical JSON v1
//! bytes, and the digest computed over them (dossier section 12.1).
//!
//! The authority hierarchy section 12.1 fixes is `YAML or JSON source -> parsed Procedure v2 model
//! -> Canonical JSON/IR -> digest`. This module owns the third arrow. Canonical form is therefore
//! never a reformatting of the source document: it is rebuilt from the validated model through its
//! public accessors, so anything the source could carry but the model does not — comments, key
//! order, block versus flow style, quoting, and omitted-versus-explicit defaults — is gone before
//! the first byte is written, and section 13.3's promise that "formatting and comments never affect
//! it" holds by construction rather than by normalization rules.
//!
//! The canonical document is authoring-shaped: it is the same closed shape
//! `assets/schemas/procedure-v2.schema.json` fixes for source documents, so a canonical document is
//! itself a valid Procedure v2 document that re-parses to the same model (the fixpoint the
//! canonical golden test asserts). Four rules fix the shape exactly:
//!
//! 1. Documented defaults are materialized. Every default `procedure_v2_wire` declares through
//!    `serde` — text `min_length`/`max_length`/`multiline`, list `min_items`/`max_items`/
//!    `max_item_length`/`unique`, and evidence-reference `required` — appears explicitly, so
//!    omitting a default and authoring it produce identical bytes.
//! 2. Absent optional scalars and empty optional collections are omitted. The schema gives every
//!    optional collection `minItems: 1` and the parser rejects an explicitly empty one, so an
//!    empty collection has no representable canonical form; `goal_tracking` is present exactly when
//!    the procedure opted in, mirroring its `const: true` schema shape.
//! 3. Author-order-meaningful arrays keep author order: `graph.nodes`, `options`, `items`,
//!    `instructions`, `evidence_guidance`, `evidence_from`, selected evidence `items`, `choices`,
//!    `allowed_media_types`, and `manual_rework.allowed_targets`. Reordering any of them is a
//!    semantic edit and changes the digest.
//! 4. Authoring maps stay maps: `node_definitions`, decision `routes`, and assessment `outcomes`
//!    become JSON objects, so Canonical JSON v1's byte-sorted key order normalizes them away and
//!    reordering their keys cannot change the digest.
//!
//! Numbers are integers only, which Canonical JSON v1 requires; the v2 model has no non-integer
//! number.

use podway_core::{
    ActionDefinitionV2, ActionOutcomeV2, AssessmentContractV2, DecisionDefinitionV2,
    EvidenceFromListV2, GraphPlacementV2, ItemCommonV2, ItemSpecV2, ProcedureGraphV2,
    SOURCE_PROJECTION_MAX_CHARACTERS, Sha256Digest,
};
use serde_json::{Map, Value};
use sha2::{Digest as _, Sha256};

use crate::procedure_v2_wire::PROCEDURE_SCHEMA_V2;
use crate::{
    CanonicalJsonV1, ConfigError, ParsedNodeDefinition, ParsedProcedureV2,
    canonical_json_from_serializable, validate_count,
};

/// Builds the canonical bytes and digest of an already closed-reference-validated model.
///
/// The projection bound of section 5.1 is enforced here because this is where the projection first
/// exists: no earlier stage can measure a document that is only produced by canonicalization. The
/// future stable diagnostic code for this rejection is `SOURCE_PROJECTION_BUDGET_EXCEEDED`
/// (sections 11.1 and 11.2); binding `ConfigError` values to the catalog in
/// `assets/specifications/authoring-diagnostics.json` is V2AUT-008's task.
pub(crate) fn canonical_projection(
    parsed: &ParsedProcedureV2,
) -> Result<(CanonicalJsonV1, Sha256Digest), ConfigError> {
    let canonical_json = canonical_json_from_serializable(&canonical_document_value(parsed))?;
    validate_count(
        "canonical source projection",
        canonical_json.as_str().chars().count(),
        1,
        SOURCE_PROJECTION_MAX_CHARACTERS,
    )?;
    let digest = Sha256Digest::new(format!(
        "sha256:{:x}",
        Sha256::digest(canonical_json.as_bytes())
    ))
    .map_err(|_| ConfigError::InvalidDigest)?;
    Ok((canonical_json, digest))
}

/// Rebuilds the authoring-shaped canonical document from the validated model.
///
/// Only the model's public accessors are read, so the canonical form can never depend on a wire
/// DTO, on source bytes, or on private parser state.
pub(crate) fn canonical_document_value(parsed: &ParsedProcedureV2) -> Value {
    let mut document = Map::new();
    insert(&mut document, "schema", PROCEDURE_SCHEMA_V2);
    insert(&mut document, "id", parsed.id());
    insert(&mut document, "version", parsed.version());
    insert(&mut document, "name", parsed.name());
    insert(&mut document, "purpose", parsed.purpose());
    insert_some(&mut document, "description", text(parsed.description()));
    // `goal_tracking` is a `const: true` opt-in: present iff enabled, never authored as `false`.
    if parsed
        .goal_tracking()
        .is_some_and(|opt_in| opt_in.is_enabled())
    {
        document.insert("goal_tracking".to_owned(), Value::Bool(true));
    }

    let mut definitions = Map::new();
    for definition in parsed.node_definitions() {
        definitions.insert(
            definition.id().as_str().to_owned(),
            node_definition_value(definition),
        );
    }
    document.insert("node_definitions".to_owned(), Value::Object(definitions));
    document.insert("graph".to_owned(), graph_value(parsed.graph()));
    // `manual_rework` is authored at the document root; the model hangs it off the graph.
    if let Some(manual_rework) = parsed.graph().manual_rework() {
        let mut rework = Map::new();
        rework.insert(
            "allowed_targets".to_owned(),
            strings(manual_rework.targets().iter().map(|target| target.as_str())),
        );
        document.insert("manual_rework".to_owned(), Value::Object(rework));
    }
    Value::Object(document)
}

fn node_definition_value(definition: &ParsedNodeDefinition) -> Value {
    match definition {
        ParsedNodeDefinition::Action(action) => action_definition_value(action),
        ParsedNodeDefinition::Decision(decision) => decision_definition_value(decision),
    }
}

fn action_definition_value(definition: &ActionDefinitionV2) -> Value {
    let mut value = Map::new();
    insert(&mut value, "type", "action");
    insert(&mut value, "title", definition.title());
    insert(&mut value, "intent", definition.intent());
    insert_some(&mut value, "description", text(definition.description()));
    insert_some(
        &mut value,
        "instructions",
        non_empty_strings(definition.instructions()),
    );
    insert_some(&mut value, "items", items_value(definition.items()));
    Value::Object(value)
}

fn decision_definition_value(definition: &DecisionDefinitionV2) -> Value {
    let mut value = Map::new();
    insert(&mut value, "type", "decision");
    insert(&mut value, "title", definition.title());
    insert_some(&mut value, "description", text(definition.description()));
    insert(&mut value, "objective", definition.objective());
    insert(&mut value, "prompt", definition.prompt());
    insert_some(
        &mut value,
        "evidence_guidance",
        non_empty_strings(definition.evidence_guidance()),
    );
    insert_some(&mut value, "items", items_value(definition.items()));

    let options = definition
        .options()
        .iter()
        .map(|option| {
            let mut entry = Map::new();
            insert(&mut entry, "id", option.id().as_str());
            insert(&mut entry, "label", option.label());
            insert_some(&mut entry, "criteria", text(option.criteria()));
            Value::Object(entry)
        })
        .collect::<Vec<_>>();
    value.insert("options".to_owned(), Value::Array(options));

    let mut reason = Map::new();
    // A declared reason policy always carries `required: true`; the constructor rejects `false`.
    reason.insert(
        "required".to_owned(),
        Value::Bool(definition.reason().required()),
    );
    insert_some(&mut reason, "prompt", text(definition.reason().prompt()));
    value.insert("reason".to_owned(), Value::Object(reason));

    insert_some(
        &mut value,
        "assessment",
        definition.assessment().map(assessment_value),
    );
    Value::Object(value)
}

fn assessment_value(assessment: &AssessmentContractV2) -> Value {
    let mut value = Map::new();
    insert(&mut value, "target", assessment.target().as_str());
    let mut outcomes = Map::new();
    for mapping in assessment.outcomes() {
        outcomes.insert(
            mapping.option_id().as_str().to_owned(),
            Value::String(mapping.outcome().as_str().to_owned()),
        );
    }
    value.insert("outcomes".to_owned(), Value::Object(outcomes));
    Value::Object(value)
}

fn items_value(items: &[ItemSpecV2]) -> Option<Value> {
    (!items.is_empty()).then(|| Value::Array(items.iter().map(item_value).collect()))
}

fn item_value(item: &ItemSpecV2) -> Value {
    let mut value = Map::new();
    common_item_fields(&mut value, item.common());
    match item {
        ItemSpecV2::Confirm(_) => insert(&mut value, "type", "confirm"),
        ItemSpecV2::Text(specification) => {
            insert(&mut value, "type", "text");
            // Materialized defaults: min_length 0, max_length 4,000, multiline true.
            value.insert(
                "min_length".to_owned(),
                Value::from(specification.min_length()),
            );
            value.insert(
                "max_length".to_owned(),
                Value::from(specification.max_length()),
            );
            value.insert(
                "multiline".to_owned(),
                Value::Bool(specification.multiline()),
            );
        }
        ItemSpecV2::Choice(specification) => {
            insert(&mut value, "type", "choice");
            value.insert(
                "choices".to_owned(),
                strings(specification.choices().iter().map(String::as_str)),
            );
        }
        ItemSpecV2::Integer(specification) => {
            insert(&mut value, "type", "integer");
            // `minimum`/`maximum` have no documented default: absent stays absent.
            insert_some(
                &mut value,
                "minimum",
                specification.minimum().map(Value::from),
            );
            insert_some(
                &mut value,
                "maximum",
                specification.maximum().map(Value::from),
            );
        }
        ItemSpecV2::List(specification) => {
            insert(&mut value, "type", "list");
            // Materialized defaults: min_items 0, max_items 50, max_item_length 500, unique true.
            value.insert(
                "min_items".to_owned(),
                Value::from(specification.min_items()),
            );
            value.insert(
                "max_items".to_owned(),
                Value::from(specification.max_items()),
            );
            value.insert(
                "max_item_length".to_owned(),
                Value::from(specification.max_item_length()),
            );
            value.insert("unique".to_owned(), Value::Bool(specification.unique()));
        }
        ItemSpecV2::Artifact(specification) => {
            insert(&mut value, "type", "artifact");
            insert_some(
                &mut value,
                "allowed_media_types",
                non_empty_strings(specification.allowed_media_types()),
            );
        }
    }
    Value::Object(value)
}

fn common_item_fields(value: &mut Map<String, Value>, common: &ItemCommonV2) {
    insert(value, "id", common.id().as_str());
    insert(value, "prompt", common.prompt());
    insert_some(value, "help", text(common.help()));
    value.insert("required".to_owned(), Value::Bool(common.required()));
}

fn graph_value(graph: &ProcedureGraphV2) -> Value {
    let mut value = Map::new();
    insert(&mut value, "entry", graph.entry().as_str());
    value.insert(
        "nodes".to_owned(),
        Value::Array(graph.placements().iter().map(placement_value).collect()),
    );
    Value::Object(value)
}

fn placement_value(placement: &GraphPlacementV2) -> Value {
    let mut value = Map::new();
    match placement {
        GraphPlacementV2::Action(action) => {
            insert(&mut value, "id", action.id().as_str());
            insert(&mut value, "use", action.definition().as_str());
            insert_some(
                &mut value,
                "evidence_from",
                action.evidence_from().map(evidence_from_value),
            );
            if let Some(skip) = action.skip() {
                let mut policy = Map::new();
                // A declared skip policy always carries `allowed: true`; `false` is rejected.
                policy.insert("allowed".to_owned(), Value::Bool(skip.is_allowed()));
                policy.insert(
                    "reason_required".to_owned(),
                    Value::Bool(skip.reason_required()),
                );
                value.insert("skip".to_owned(), Value::Object(policy));
            }
            match action.outcome() {
                ActionOutcomeV2::Next(target) => insert(&mut value, "next", target.as_str()),
                ActionOutcomeV2::Terminal => {
                    value.insert("terminal".to_owned(), Value::Bool(true));
                }
            }
        }
        GraphPlacementV2::Decision(decision) => {
            insert(&mut value, "id", decision.id().as_str());
            insert(&mut value, "use", decision.definition().as_str());
            insert_some(
                &mut value,
                "evidence_from",
                decision.evidence_from().map(evidence_from_value),
            );
            let mut routes = Map::new();
            for entry in decision.routes().entries() {
                let mut route = Map::new();
                insert(&mut route, "to", entry.route().to().as_str());
                insert(&mut route, "effect", entry.route().effect().as_str());
                routes.insert(entry.option_id().as_str().to_owned(), Value::Object(route));
            }
            value.insert("routes".to_owned(), Value::Object(routes));
        }
    }
    Value::Object(value)
}

fn evidence_from_value(entries: &EvidenceFromListV2) -> Value {
    let references = entries
        .entries()
        .iter()
        .map(|reference| {
            let mut value = Map::new();
            insert(&mut value, "node", reference.source_node().as_str());
            // Materialized default: `required` is true when omitted.
            value.insert("required".to_owned(), Value::Bool(reference.required()));
            insert_some(
                &mut value,
                "items",
                reference
                    .selected_items()
                    .map(|items| strings(items.iter().map(|item| item.as_str()))),
            );
            Value::Object(value)
        })
        .collect::<Vec<_>>();
    Value::Array(references)
}

fn insert(target: &mut Map<String, Value>, key: &str, value: &str) {
    target.insert(key.to_owned(), Value::String(value.to_owned()));
}

fn insert_some(target: &mut Map<String, Value>, key: &str, value: Option<Value>) {
    if let Some(value) = value {
        target.insert(key.to_owned(), value);
    }
}

fn text(value: Option<&str>) -> Option<Value> {
    value.map(|value| Value::String(value.to_owned()))
}

fn strings<'a>(values: impl Iterator<Item = &'a str>) -> Value {
    Value::Array(
        values
            .map(|value| Value::String(value.to_owned()))
            .collect(),
    )
}

fn non_empty_strings(values: &[String]) -> Option<Value> {
    (!values.is_empty()).then(|| strings(values.iter().map(String::as_str)))
}
