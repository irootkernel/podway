use podway_core::{EvidenceFromListV2, GraphPlacementV2};

use crate::procedure_v2_source::FieldPath;

pub(crate) fn definition_path(id: &str) -> FieldPath {
    FieldPath::root()
        .child_key("node_definitions")
        .child_key(id)
}

pub(crate) fn node_path(index: usize) -> FieldPath {
    FieldPath::root()
        .child_key("graph")
        .child_key("nodes")
        .child_index(index)
}

pub(crate) fn placement_definition_id(placement: &GraphPlacementV2) -> &str {
    match placement {
        GraphPlacementV2::Action(action) => action.definition().as_str(),
        GraphPlacementV2::Decision(decision) => decision.definition().as_str(),
    }
}

pub(crate) fn placement_evidence_from(placement: &GraphPlacementV2) -> Option<&EvidenceFromListV2> {
    match placement {
        GraphPlacementV2::Action(action) => action.evidence_from(),
        GraphPlacementV2::Decision(decision) => decision.evidence_from(),
    }
}
