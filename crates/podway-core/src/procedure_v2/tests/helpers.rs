//! Shared constructor helpers reused across the redistributed v2 unit tests.

use crate::{ItemCommonV2, ItemId, ItemSpecV2, NodeDefinitionId, OptionId};

pub(super) fn def_id(value: &str) -> NodeDefinitionId {
    NodeDefinitionId::new(value).unwrap()
}

pub(super) fn opt_id(value: &str) -> OptionId {
    OptionId::new(value).unwrap()
}

pub(super) fn item(id: &str) -> ItemSpecV2 {
    ItemSpecV2::confirm(
        ItemCommonV2::new(
            ItemId::new(id).unwrap(),
            format!("Prompt for {id}"),
            None,
            true,
        )
        .unwrap(),
    )
}
