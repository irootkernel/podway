//! Procedure v2 wire DTOs and the order-preserving mapping helper.
//!
//! These mirror the closed authoring shape fixed by `assets/schemas/procedure-v2.schema.json`
//! and dossier section 5: every object uses `deny_unknown_fields`, documented defaults are
//! declared through `serde`, and the authoring maps (`node_definitions`, decision `routes`,
//! assessment `outcomes`) flow through [`OrderedMap`] so source order is preserved while the
//! shared bounded YAML decoder rejects duplicate keys, aliases, tags, and oversized input.

use std::fmt;
use std::marker::PhantomData;

use serde::Deserialize;
use serde::de::{Deserializer, MapAccess, Visitor};

/// The exact Procedure v2 schema identifier.
pub(crate) const PROCEDURE_SCHEMA_V2: &str = podway_core::PROCEDURE_SCHEMA_V2;

const DEFAULT_TEXT_MAX_LENGTH: u32 = 4_000;
const DEFAULT_LIST_MAX_ITEMS: u16 = 50;
const DEFAULT_LIST_MAX_ITEM_LENGTH: u16 = 500;

const fn default_text_max_length() -> u32 {
    DEFAULT_TEXT_MAX_LENGTH
}
const fn default_list_max_items() -> u16 {
    DEFAULT_LIST_MAX_ITEMS
}
const fn default_list_max_item_length() -> u16 {
    DEFAULT_LIST_MAX_ITEM_LENGTH
}
const fn default_true() -> bool {
    true
}

/// A YAML mapping captured as author-ordered `(key, value)` pairs.
///
/// Deserializing through `serde_json::Value` would sort mapping keys alphabetically (the
/// workspace builds `serde_json` without `preserve_order`) and lose author order for
/// `node_definitions`, `routes`, and `outcomes`; this newtype drives a `MapAccess` loop
/// directly, leaving duplicate-key, alias, tag, and bound rejection to the shared decoder.
#[derive(Clone, Debug)]
pub(crate) struct OrderedMap<K, V> {
    entries: Vec<(K, V)>,
}

impl<K, V> OrderedMap<K, V> {
    /// Builds a map from pairs that are already in the order they must be authored in.
    ///
    /// The seam `procedure_v2_convert` needs: a converted document is constructed in memory rather
    /// than deserialized, and it must reach [`crate::procedure_v2_parse::map_document`] through the
    /// same DTO an authored document does, so the conversion inherits every bound check instead of
    /// re-implementing one.
    pub(crate) fn from_entries(entries: Vec<(K, V)>) -> Self {
        Self { entries }
    }

    pub(crate) fn entries(self) -> Vec<(K, V)> {
        self.entries
    }
}

impl<'de, K, V> Deserialize<'de> for OrderedMap<K, V>
where
    K: Deserialize<'de>,
    V: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct OrderedMapVisitor<K, V>(PhantomData<(K, V)>);

        impl<'de, K, V> Visitor<'de> for OrderedMapVisitor<K, V>
        where
            K: Deserialize<'de>,
            V: Deserialize<'de>,
        {
            type Value = Vec<(K, V)>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a mapping")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut entries = Vec::new();
                while let Some(key) = map.next_key::<K>()? {
                    let value = map.next_value::<V>()?;
                    entries.push((key, value));
                }
                Ok(entries)
            }
        }

        let entries = deserializer.deserialize_map(OrderedMapVisitor(PhantomData))?;
        Ok(Self { entries })
    }
}

/// The top-level Procedure v2 authoring document.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ProcedureV2DocumentWire {
    pub(crate) schema: String,
    pub(crate) id: String,
    pub(crate) version: String,
    pub(crate) name: String,
    pub(crate) purpose: String,
    #[serde(default)]
    pub(crate) description: Option<String>,
    #[serde(default)]
    pub(crate) goal_tracking: Option<bool>,
    pub(crate) node_definitions: OrderedMap<String, NodeDefinitionWire>,
    pub(crate) graph: GraphWire,
    #[serde(default)]
    pub(crate) manual_rework: Option<ManualReworkWire>,
}

/// A reusable node definition, discriminated by its closed `type` tag.
#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
pub(crate) enum NodeDefinitionWire {
    Action {
        title: String,
        intent: String,
        #[serde(default)]
        description: Option<String>,
        #[serde(default)]
        instructions: Option<Vec<String>>,
        #[serde(default)]
        items: Option<Vec<ItemWire>>,
    },
    Decision {
        title: String,
        #[serde(default)]
        description: Option<String>,
        objective: String,
        prompt: String,
        #[serde(default)]
        evidence_guidance: Option<Vec<String>>,
        #[serde(default)]
        items: Option<Vec<ItemWire>>,
        options: Vec<DecisionOptionWire>,
        reason: ReasonPolicyWire,
        #[serde(default)]
        assessment: Option<AssessmentWire>,
    },
}

/// One recorded-item specification, discriminated by its closed `type` tag.
#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase", deny_unknown_fields)]
pub(crate) enum ItemWire {
    Confirm {
        id: String,
        prompt: String,
        #[serde(default)]
        help: Option<String>,
        required: bool,
    },
    Text {
        id: String,
        prompt: String,
        #[serde(default)]
        help: Option<String>,
        required: bool,
        #[serde(default)]
        min_length: u32,
        #[serde(default = "default_text_max_length")]
        max_length: u32,
        #[serde(default = "default_true")]
        multiline: bool,
    },
    Choice {
        id: String,
        prompt: String,
        #[serde(default)]
        help: Option<String>,
        required: bool,
        choices: Vec<String>,
    },
    Integer {
        id: String,
        prompt: String,
        #[serde(default)]
        help: Option<String>,
        required: bool,
        #[serde(default)]
        minimum: Option<i64>,
        #[serde(default)]
        maximum: Option<i64>,
    },
    List {
        id: String,
        prompt: String,
        #[serde(default)]
        help: Option<String>,
        required: bool,
        #[serde(default)]
        min_items: u16,
        #[serde(default = "default_list_max_items")]
        max_items: u16,
        #[serde(default = "default_list_max_item_length")]
        max_item_length: u16,
        #[serde(default = "default_true")]
        unique: bool,
    },
    Artifact {
        id: String,
        prompt: String,
        #[serde(default)]
        help: Option<String>,
        required: bool,
        #[serde(default)]
        allowed_media_types: Option<Vec<String>>,
    },
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct DecisionOptionWire {
    pub(crate) id: String,
    pub(crate) label: String,
    #[serde(default)]
    pub(crate) criteria: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReasonPolicyWire {
    pub(crate) required: bool,
    #[serde(default)]
    pub(crate) prompt: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AssessmentWire {
    pub(crate) target: String,
    pub(crate) outcomes: OrderedMap<String, String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GraphWire {
    pub(crate) entry: String,
    pub(crate) nodes: Vec<GraphPlacementWire>,
}

/// One placed graph node. The action/decision distinction (`routes` versus `next`/`terminal`)
/// is resolved during mapping so the combined shape keeps `deny_unknown_fields` effective and
/// avoids `serde` content buffering that would lose mapping order.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GraphPlacementWire {
    pub(crate) id: String,
    #[serde(rename = "use")]
    pub(crate) use_: String,
    #[serde(default)]
    pub(crate) evidence_from: Option<Vec<EvidenceReferenceWire>>,
    #[serde(default)]
    pub(crate) skip: Option<SkipPolicyWire>,
    #[serde(default)]
    pub(crate) next: Option<String>,
    #[serde(default)]
    pub(crate) terminal: Option<bool>,
    #[serde(default)]
    pub(crate) routes: Option<OrderedMap<String, RouteWire>>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct EvidenceReferenceWire {
    pub(crate) node: String,
    #[serde(default = "default_true")]
    pub(crate) required: bool,
    #[serde(default)]
    pub(crate) items: Option<Vec<String>>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SkipPolicyWire {
    pub(crate) allowed: bool,
    pub(crate) reason_required: bool,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RouteWire {
    pub(crate) to: String,
    pub(crate) effect: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ManualReworkWire {
    pub(crate) allowed_targets: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordered_map_preserves_source_order_for_arbitrary_keys() {
        let yaml = "beta: 2\nalpha: 1\ngamma: 3\n";
        let map: OrderedMap<String, u32> = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(
            map.entries(),
            vec![
                ("beta".to_owned(), 2),
                ("alpha".to_owned(), 1),
                ("gamma".to_owned(), 3),
            ]
        );
    }
}
