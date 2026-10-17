//! Engine `item.record_many` proof of the attempt-content rejection.
//!
//! The canonical mutation identity of each seed and rejection stays within the
//! 1,048,576-byte request limit.

use podway_core::{Revision, WorkspaceId};
use podway_engine::direct_api::{DirectCommandV1, DirectResponseV1};
use podway_protocol::{
    ItemRecordManyV1, SliceCommandV1, canonical_direct_slice_mutation_identity_v1,
    decode_item_record_many_operations_v1,
};
use podway_store::{GraphSessionStateV2, IdempotencyKeyV1};
use serde_json::{Value, json};

use super::int_v2dfc006_direct_execution::{Clock, deadline};
use super::int_v2dfc006_runtime_ports::Engine;

const REQUEST_LIMIT_BYTES: usize = 1_048_576;
const WORKSPACE: &str = "00000000-0000-4000-8000-000000000001";

fn item_ids() -> Vec<String> {
    let mut ids = vec!["a-early".to_owned()];
    ids.extend((0..15).map(|index| format!("b{index:02}")));
    ids.push("z-later".to_owned());
    ids
}

fn overflow_procedure() -> String {
    let items = item_ids()
        .into_iter()
        .map(|id| {
            format!(
                "      - id: {id}\n        type: list\n        prompt: Record {id}.\n        required: false\n        max_items: 1000\n        max_item_length: 1000\n        max_total_length: 1000000\n        unique: false\n"
            )
        })
        .collect::<String>();
    format!(
        "schema: podway.procedure/v2
id: optim-overflow
version: \"1\"
name: Optim overflow
purpose: Seed an attempt within one legal list of the scalar ceiling.
node_definitions:
  work:
    type: action
    title: Record lists
    intent: Record bounded lists.
    items:
{items}graph:
  entry: work
  nodes:
    - id: work
      use: work
      terminal: true
"
    )
}

fn list_values(entries: usize, width: usize) -> Vec<String> {
    vec!["a".repeat(width); entries]
}

fn slot_revision(graph: &GraphSessionStateV2, item: &str) -> Revision {
    let attempt = graph.trace().active_attempt().unwrap().attempt_id();
    graph
        .workflow_memory()
        .attempts()
        .iter()
        .find(|memory| memory.attempt_id() == attempt)
        .unwrap()
        .item_slots()
        .iter()
        .find(|slot| slot.item_id().as_str() == item)
        .unwrap()
        .revision()
}

fn slot_revisions(graph: &GraphSessionStateV2) -> Vec<(String, u64)> {
    let attempt = graph.trace().active_attempt().unwrap().attempt_id();
    graph
        .workflow_memory()
        .attempts()
        .iter()
        .find(|memory| memory.attempt_id() == attempt)
        .unwrap()
        .item_slots()
        .iter()
        .map(|slot| (slot.item_id().as_str().to_owned(), slot.revision().get()))
        .collect()
}

fn content_scalars(graph: &GraphSessionStateV2) -> u64 {
    graph
        .workflow_memory()
        .attempts()
        .iter()
        .map(podway_store::AttemptWorkflowMemoryV2::content_scalars)
        .sum()
}

fn record_command(engine: &Engine, operations: Value) -> DirectCommandV1 {
    DirectCommandV1::RecordMany(ItemRecordManyV1 {
        operations: decode_item_record_many_operations_v1(operations).unwrap(),
        preconditions: engine.fences(),
    })
}

fn identity_bytes(command: &DirectCommandV1) -> usize {
    let DirectCommandV1::RecordMany(item) = command else {
        panic!("overflow proof submits item.record_many");
    };
    canonical_direct_slice_mutation_identity_v1(
        &SliceCommandV1::ItemRecordMany(item.clone()),
        &WorkspaceId::new(WORKSPACE).unwrap(),
    )
    .unwrap()
    .len()
}

fn outcome_absent(engine: &Engine, key: &str) -> bool {
    engine
        .store
        .read(deadline(), |snapshot| {
            snapshot.outcome(&IdempotencyKeyV1::new(key).unwrap())
        })
        .unwrap()
        .is_none()
}

#[test]
fn optim001_engine_record_many_rejects_attempt_content_without_outcome() {
    let engine = Engine::new(&overflow_procedure());
    let full = list_values(1000, 1000);
    for id in item_ids().into_iter().filter(|id| id != "a-early") {
        let revision = slot_revision(&engine.graph(), &id).get();
        let command = record_command(
            &engine,
            json!([{
                "item_id": id,
                "expected_item_revision": revision,
                "record": {"type": "list", "value": full.clone()}
            }]),
        );
        assert!(identity_bytes(&command) <= REQUEST_LIMIT_BYTES);
        engine.success(command, &format!("seed-{id}"));
    }
    let preimage = engine.graph();
    assert_eq!(content_scalars(&preimage), 16_000_000);
    let session_revision = preimage.trace().revision();
    let workspace_revision = preimage.workspace_revision();
    let item_revisions = slot_revisions(&preimage);
    let mut over = list_values(777, 1000);
    over.push("a".repeat(217));
    for (key, later_first) in [("reject-later-early", true), ("reject-early-later", false)] {
        let early = json!({
            "item_id": "a-early",
            "expected_item_revision": slot_revision(&preimage, "a-early").get(),
            "record": {"type": "list", "value": over.clone()}
        });
        let later = json!({
            "item_id": "z-later",
            "expected_item_revision": slot_revision(&preimage, "z-later").get(),
            "clear": true
        });
        let operations = if later_first {
            json!([later, early])
        } else {
            json!([early, later])
        };
        let command = record_command(&engine, operations);
        assert!(identity_bytes(&command) <= REQUEST_LIMIT_BYTES);
        let DirectResponseV1::Failure(error) = engine.execute(engine.request(command, key), &Clock)
        else {
            panic!("record_many overflow committed {key}");
        };
        let value = error.into_value();
        assert_eq!(value["code"], "ATTEMPT_CONTENT_LIMIT_EXCEEDED");
        assert_eq!(value["details"]["kind"], "ATTEMPT_CONTENT_LIMIT_EXCEEDED");
        assert_eq!(value["details"]["field"], "attempt recorded content");
        assert_eq!(value["details"]["actual"], 16_777_217);
        assert_eq!(value["details"]["maximum"], 16_777_216);
        assert_eq!(value["details"]["unit"], "scalars");
        let after = engine.graph();
        assert_eq!(slot_revisions(&after), item_revisions);
        assert_eq!(after.trace().revision(), session_revision);
        assert_eq!(after.workspace_revision(), workspace_revision);
        assert_eq!(after, preimage);
        assert!(outcome_absent(&engine, key));
    }
}
