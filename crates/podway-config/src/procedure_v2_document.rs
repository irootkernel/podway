//! The authoring-shaped Procedure v2 document, rebuilt from a parsed model as an order-preserving
//! tree (dossier sections 11.1 and 12.1).
//!
//! One tree, two consumers. `procedure_v2_canonical` turns it into `serde_json::Value` for the
//! Canonical JSON/IR digest; `procedure_v2_format` renders it as canonical authoring YAML or JSON.
//! Because both read the same value, formatting cannot change the digest: there is no second copy
//! of the materialize/omit rules to drift from the first.
//!
//! Two properties distinguish this tree from the `serde_json::Value` it produces:
//!
//! - **Maps keep author order.** `serde_json::Map` is a `BTreeMap` in this workspace (see
//!   `procedure_v2_wire`), so `node_definitions`, decision `routes`, and assessment `outcomes`
//!   would come out byte-sorted and `--write` would silently reorder the author's definitions.
//!   [`AuthoringValue::Map`] is a `Vec` of pairs in model order instead. The digest is unaffected:
//!   Canonical JSON v1 byte-sorts object keys, so insertion order is erased before hashing.
//! - **Keys are in `assets/schemas/procedure-v2.schema.json` `properties` order.** That is the
//!   authoring contract's own order and the order the shipped fixtures are written in — notably
//!   items read `id, type, prompt, help, required, <type-specific>`.
//!
//! The four canonical shape rules of `procedure_v2_canonical` hold here unchanged: documented
//! defaults are materialized, absent optional scalars and empty optional collections are omitted,
//! author-order-meaningful arrays keep author order, and authoring maps stay maps.

use podway_core::{
    ActionDefinitionV2, ActionOutcomeV2, AssessmentContractV2, DecisionDefinitionV2,
    EvidenceFromListV2, GraphPlacementV2, ItemCommonV2, ItemSpecV2, ProcedureGraphV2,
};
use serde_json::{Map, Value};

use crate::procedure_v2_wire::PROCEDURE_SCHEMA_V2;
use crate::{ParsedNodeDefinition, ParsedProcedureV2};

/// One node of the authoring document: the closed value space a Procedure v2 document can hold.
///
/// There is deliberately no null and no floating-point variant — the v2 model has neither, and
/// Canonical JSON v1 forbids the latter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum AuthoringValue {
    Text(String),
    Flag(bool),
    Integer(i64),
    Seq(Vec<AuthoringValue>),
    Map(Vec<(String, AuthoringValue)>),
}

impl AuthoringValue {
    /// Projects the tree into `serde_json::Value`. Map order is lost here by construction (the
    /// workspace builds `serde_json` without `preserve_order`), which is exactly what the digest
    /// wants and exactly why the formatter reads the tree instead of the projection.
    pub(crate) fn into_json(self) -> Value {
        match self {
            Self::Text(value) => Value::String(value),
            Self::Flag(value) => Value::Bool(value),
            Self::Integer(value) => Value::Number(value.into()),
            Self::Seq(values) => Value::Array(values.into_iter().map(Self::into_json).collect()),
            Self::Map(entries) => Value::Object(
                entries
                    .into_iter()
                    .map(|(key, value)| (key, value.into_json()))
                    .collect::<Map<_, _>>(),
            ),
        }
    }
}

/// Accumulates one object's entries in schema key order, skipping absent optionals.
struct Entries(Vec<(String, AuthoringValue)>);

impl Entries {
    fn new() -> Self {
        Self(Vec::new())
    }

    fn text(&mut self, key: &str, value: &str) -> &mut Self {
        self.0
            .push((key.to_owned(), AuthoringValue::Text(value.to_owned())));
        self
    }

    fn optional_text(&mut self, key: &str, value: Option<&str>) -> &mut Self {
        if let Some(value) = value {
            self.text(key, value);
        }
        self
    }

    fn flag(&mut self, key: &str, value: bool) -> &mut Self {
        self.0.push((key.to_owned(), AuthoringValue::Flag(value)));
        self
    }

    fn integer(&mut self, key: &str, value: impl Into<i64>) -> &mut Self {
        self.0
            .push((key.to_owned(), AuthoringValue::Integer(value.into())));
        self
    }

    fn optional_integer(&mut self, key: &str, value: Option<i64>) -> &mut Self {
        if let Some(value) = value {
            self.integer(key, value);
        }
        self
    }

    fn value(&mut self, key: &str, value: AuthoringValue) -> &mut Self {
        self.0.push((key.to_owned(), value));
        self
    }

    fn optional_value(&mut self, key: &str, value: Option<AuthoringValue>) -> &mut Self {
        if let Some(value) = value {
            self.value(key, value);
        }
        self
    }

    /// Omits an empty optional collection: the schema gives every optional collection
    /// `minItems: 1` and the parser rejects an explicitly empty one, so an empty collection has no
    /// representable authoring form.
    fn optional_strings<'a>(
        &mut self,
        key: &str,
        values: impl IntoIterator<Item = &'a str>,
    ) -> &mut Self {
        let values = strings(values);
        if let AuthoringValue::Seq(entries) = &values
            && entries.is_empty()
        {
            return self;
        }
        self.value(key, values)
    }

    fn finish(&mut self) -> AuthoringValue {
        AuthoringValue::Map(std::mem::take(&mut self.0))
    }
}

fn strings<'a>(values: impl IntoIterator<Item = &'a str>) -> AuthoringValue {
    AuthoringValue::Seq(
        values
            .into_iter()
            .map(|value| AuthoringValue::Text(value.to_owned()))
            .collect(),
    )
}

/// Rebuilds the complete authoring document from the validated model, in schema key order and
/// author map order.
///
/// Only the model's public accessors are read, so the document can never depend on a wire DTO, on
/// source bytes, or on private parser state.
pub(crate) fn authoring_document_value(parsed: &ParsedProcedureV2) -> AuthoringValue {
    let mut document = Entries::new();
    document
        .text("schema", PROCEDURE_SCHEMA_V2)
        .text("id", parsed.id())
        .text("version", parsed.version())
        .text("name", parsed.name())
        .text("purpose", parsed.purpose())
        .optional_text("description", parsed.description());
    // `goal_tracking` is a `const: true` opt-in: present iff enabled, never authored as `false`.
    if parsed
        .goal_tracking()
        .is_some_and(|opt_in| opt_in.is_enabled())
    {
        document.flag("goal_tracking", true);
    }

    let definitions = parsed
        .node_definitions()
        .iter()
        .map(|definition| {
            (
                definition.id().as_str().to_owned(),
                node_definition_value(definition),
            )
        })
        .collect::<Vec<_>>();
    document
        .value("node_definitions", AuthoringValue::Map(definitions))
        .value("graph", graph_value(parsed.graph()));

    // `manual_rework` is authored at the document root; the model hangs it off the graph.
    if let Some(manual_rework) = parsed.graph().manual_rework() {
        let mut rework = Entries::new();
        rework.value(
            "allowed_targets",
            strings(manual_rework.targets().iter().map(|target| target.as_str())),
        );
        document.value("manual_rework", rework.finish());
    }
    document.finish()
}

fn node_definition_value(definition: &ParsedNodeDefinition) -> AuthoringValue {
    match definition {
        ParsedNodeDefinition::Action(action) => action_definition_value(action),
        ParsedNodeDefinition::Decision(decision) => decision_definition_value(decision),
    }
}

fn action_definition_value(definition: &ActionDefinitionV2) -> AuthoringValue {
    let mut value = Entries::new();
    value
        .text("type", "action")
        .text("title", definition.title())
        .text("intent", definition.intent())
        .optional_text("description", definition.description())
        .optional_strings(
            "instructions",
            definition.instructions().iter().map(String::as_str),
        )
        .optional_value("items", items_value(definition.items()));
    value.finish()
}

fn decision_definition_value(definition: &DecisionDefinitionV2) -> AuthoringValue {
    let options = definition
        .options()
        .iter()
        .map(|option| {
            let mut entry = Entries::new();
            entry
                .text("id", option.id().as_str())
                .text("label", option.label())
                .optional_text("criteria", option.criteria());
            entry.finish()
        })
        .collect::<Vec<_>>();

    let mut reason = Entries::new();
    // A declared reason policy always carries `required: true`; the constructor rejects `false`.
    reason
        .flag("required", definition.reason().required())
        .optional_text("prompt", definition.reason().prompt());

    let mut value = Entries::new();
    value
        .text("type", "decision")
        .text("title", definition.title())
        .optional_text("description", definition.description())
        .text("objective", definition.objective())
        .text("prompt", definition.prompt())
        .optional_strings(
            "evidence_guidance",
            definition.evidence_guidance().iter().map(String::as_str),
        )
        .optional_value("items", items_value(definition.items()))
        .value("options", AuthoringValue::Seq(options))
        .value("reason", reason.finish())
        .optional_value("assessment", definition.assessment().map(assessment_value));
    value.finish()
}

fn assessment_value(assessment: &AssessmentContractV2) -> AuthoringValue {
    let outcomes = assessment
        .outcomes()
        .iter()
        .map(|mapping| {
            (
                mapping.option_id().as_str().to_owned(),
                AuthoringValue::Text(mapping.outcome().as_str().to_owned()),
            )
        })
        .collect::<Vec<_>>();
    let mut value = Entries::new();
    value
        .text("target", assessment.target().as_str())
        .value("outcomes", AuthoringValue::Map(outcomes));
    value.finish()
}

fn items_value(items: &[ItemSpecV2]) -> Option<AuthoringValue> {
    (!items.is_empty())
        .then(|| AuthoringValue::Seq(items.iter().map(item_value).collect::<Vec<_>>()))
}

fn item_value(item: &ItemSpecV2) -> AuthoringValue {
    let mut value = Entries::new();
    value.text("id", item.common().id().as_str());
    match item {
        ItemSpecV2::Confirm(_) => {
            value.text("type", "confirm");
            common_item_fields(&mut value, item.common());
        }
        ItemSpecV2::Text(specification) => {
            value.text("type", "text");
            common_item_fields(&mut value, item.common());
            // Materialized defaults: min_length 0, max_length 4,000, multiline true.
            value
                .integer("min_length", specification.min_length())
                .integer("max_length", specification.max_length())
                .flag("multiline", specification.multiline());
        }
        ItemSpecV2::Choice(specification) => {
            value.text("type", "choice");
            common_item_fields(&mut value, item.common());
            value.value(
                "choices",
                strings(specification.choices().iter().map(String::as_str)),
            );
        }
        ItemSpecV2::Integer(specification) => {
            value.text("type", "integer");
            common_item_fields(&mut value, item.common());
            // `minimum`/`maximum` have no documented default: absent stays absent.
            value
                .optional_integer("minimum", specification.minimum())
                .optional_integer("maximum", specification.maximum());
        }
        ItemSpecV2::List(specification) => {
            value.text("type", "list");
            common_item_fields(&mut value, item.common());
            // Materialized defaults: min_items 0, max_items 50, max_item_length 500, unique true.
            value
                .integer("min_items", specification.min_items())
                .integer("max_items", specification.max_items())
                .integer("max_item_length", specification.max_item_length())
                .flag("unique", specification.unique());
        }
        ItemSpecV2::Artifact(specification) => {
            value.text("type", "artifact");
            common_item_fields(&mut value, item.common());
            value.optional_strings(
                "allowed_media_types",
                specification
                    .allowed_media_types()
                    .iter()
                    .map(String::as_str),
            );
        }
    }
    value.finish()
}

/// The `prompt, help?, required` run every item shares, written after `id` and `type` so the
/// object reads in `assets/schemas/procedure-v2.schema.json` `properties` order.
fn common_item_fields(value: &mut Entries, common: &ItemCommonV2) {
    value
        .text("prompt", common.prompt())
        .optional_text("help", common.help())
        .flag("required", common.required());
}

fn graph_value(graph: &ProcedureGraphV2) -> AuthoringValue {
    let mut value = Entries::new();
    value.text("entry", graph.entry().as_str()).value(
        "nodes",
        AuthoringValue::Seq(graph.placements().iter().map(placement_value).collect()),
    );
    value.finish()
}

fn placement_value(placement: &GraphPlacementV2) -> AuthoringValue {
    let mut value = Entries::new();
    match placement {
        GraphPlacementV2::Action(action) => {
            value
                .text("id", action.id().as_str())
                .text("use", action.definition().as_str())
                .optional_value(
                    "evidence_from",
                    action.evidence_from().map(evidence_from_value),
                );
            if let Some(skip) = action.skip() {
                let mut policy = Entries::new();
                // A declared skip policy always carries `allowed: true`; `false` is rejected.
                policy
                    .flag("allowed", skip.is_allowed())
                    .flag("reason_required", skip.reason_required());
                value.value("skip", policy.finish());
            }
            match action.outcome() {
                ActionOutcomeV2::Next(target) => {
                    value.text("next", target.as_str());
                }
                ActionOutcomeV2::Terminal => {
                    value.flag("terminal", true);
                }
            }
        }
        GraphPlacementV2::Decision(decision) => {
            let routes = decision
                .routes()
                .entries()
                .iter()
                .map(|entry| {
                    let mut route = Entries::new();
                    route
                        .text("to", entry.route().to().as_str())
                        .text("effect", entry.route().effect().as_str());
                    (entry.option_id().as_str().to_owned(), route.finish())
                })
                .collect::<Vec<_>>();
            value
                .text("id", decision.id().as_str())
                .text("use", decision.definition().as_str())
                .optional_value(
                    "evidence_from",
                    decision.evidence_from().map(evidence_from_value),
                )
                .value("routes", AuthoringValue::Map(routes));
        }
    }
    value.finish()
}

fn evidence_from_value(entries: &EvidenceFromListV2) -> AuthoringValue {
    AuthoringValue::Seq(
        entries
            .entries()
            .iter()
            .map(|reference| {
                let mut value = Entries::new();
                value
                    .text("node", reference.source_node().as_str())
                    // Materialized default: `required` is true when omitted.
                    .flag("required", reference.required())
                    .optional_value(
                        "items",
                        reference
                            .selected_items()
                            .map(|items| strings(items.iter().map(|item| item.as_str()))),
                    );
                value.finish()
            })
            .collect(),
    )
}
