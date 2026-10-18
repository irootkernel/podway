//! READS-002 domain-valid data-access fixtures and native query checks.
//!
//! Recipes were registered before loader changes: repeated changed-slot
//! waves, non-aligned cross-attempt packing, pointer reuse and heterogeneous
//! payload overlap. Assertions advance only to the preregistered targets;
//! raw baseline measurements remain in the qualification guide. Native
//! EXPLAIN checks inspect SQL captured from the actual Store read path.

use podway_core::workflow_v2::{ActiveItemMutationRequestV2, ActiveItemMutationV2};
use podway_core::{
    AttemptId, AttemptLifecycle, AttemptNumberV2, AttemptValidityV2, CanonicalProcedureJsonV1,
    GraphNodeId, ItemId, ItemTypeV1, ProcedureSnapshotId, ProcedureSourceLabelV1,
    RecordedItemValueV2, Revision, SessionAttemptV2, SessionId, SessionLifecycle, SessionTraceV2,
    Sha256Digest, TraceSequenceV2, UnixMillis, WorkspaceId, canonicalize_json_v1,
    recorded_value_encoded_size_v2,
};
use podway_store::test_support::query_account::{
    QueryAccountant, clear_payload_registry, declare_cached_input_bytes, payload_bytes_of,
    payload_pointer,
};
use podway_store::{
    AttemptMetadataV2, DurableWorktreeIdentityV1, GraphNodeCounterV2, GraphSessionStateV2,
    ItemSlotStateV2, ProcedureSnapshotV2,
};
use serde_json::json;
use sha2::{Digest, Sha256};

use super::int_v2dfc005_direct_store::{deadline, digest, request, store, success};
use super::int_v2dfc008_persistence::{fixture, seed};

/// A work-only procedure chain: `node_items[i]` text items on node i, the
/// last node terminal. Every node has its own definition so item counts can
/// vary per node (the non-aligned global-packing shape).
pub(super) fn workchain_snapshot(
    node_items: &[usize],
    snapshot_number: u64,
) -> ProcedureSnapshotV2 {
    workchain_snapshot_inner(node_items, snapshot_number, false)
}

/// Variant whose LAST node declares its items in reverse id order, so
/// declaration order genuinely differs from the database's item_id ASC
/// delivery order.
fn workchain_snapshot_reversed_last(
    node_items: &[usize],
    snapshot_number: u64,
) -> ProcedureSnapshotV2 {
    workchain_snapshot_inner(node_items, snapshot_number, true)
}

fn workchain_snapshot_inner(
    node_items: &[usize],
    snapshot_number: u64,
    reverse_last: bool,
) -> ProcedureSnapshotV2 {
    let items = |count: usize| {
        (0..count)
            .map(|index| {
                json!({
                    "id": format!("item-{index}"),
                    "type": "text",
                    "prompt": format!("Record item {index}."),
                    "required": false,
                    "min_length": 1,
                    "max_length": 65_536,
                    "multiline": false
                })
            })
            .collect::<Vec<_>>()
    };
    let mut definitions = serde_json::Map::new();
    let mut nodes = Vec::new();
    for (index, count) in node_items.iter().enumerate() {
        let definition = format!("work-def-{index}");
        definitions.insert(
            definition.clone(),
            json!({
                "type": "action",
                "title": format!("Work {index}"),
                "intent": "Record bounded values for reload baselines.",
                "items": items(*count)
            }),
        );
        let mut node = json!({"id": format!("n{index}"), "use": definition});
        if index + 1 == node_items.len() {
            node["terminal"] = json!(true);
        } else {
            node["next"] = json!(format!("n{}", index + 1));
        }
        nodes.push(node);
    }
    let mut document = json!({
        "schema": "podway.procedure/v2",
        "id": "reads002-baseline",
        "version": "1",
        "name": "READS-002 baseline",
        "purpose": "Domain-valid multi-successor reload baseline fixtures.",
        "node_definitions": definitions,
        "graph": {"entry": "n0", "nodes": nodes}
    });
    if reverse_last {
        let last = node_items.len() - 1;
        let definition = format!("work-def-{last}");
        if let Some(items) = document
            .get_mut("node_definitions")
            .and_then(|defs| defs.get_mut(&definition))
            .and_then(|def| def.get_mut("items"))
            .and_then(|items| items.as_array_mut())
        {
            items.reverse();
        }
    }
    let canonical = canonicalize_json_v1(&document).unwrap();
    let procedure_digest =
        Sha256Digest::new(format!("sha256:{:x}", Sha256::digest(canonical.as_bytes()))).unwrap();
    podway_store::admit_procedure_snapshot_v2(
        ProcedureSnapshotId::new(format!("00000000-0000-4000-8000-{snapshot_number:012x}"))
            .unwrap(),
        CanonicalProcedureJsonV1::new(canonical).unwrap(),
        procedure_digest,
        ProcedureSourceLabelV1::file("reads002.yaml").unwrap(),
        UnixMillis::new(5),
    )
    .unwrap()
}

pub(super) fn chain_attempt_uuid(session_number: u64, index: usize) -> AttemptId {
    let value = (session_number << 24) | (index as u64 & 0xff_ffff);
    AttemptId::new(format!("00000000-0000-4000-8000-{value:012x}")).unwrap()
}

pub(super) fn chain_session_id(session_number: u64) -> SessionId {
    SessionId::new(format!("00000000-0000-4000-8000-{session_number:012x}")).unwrap()
}

fn chain_item_id(index: usize) -> ItemId {
    ItemId::new(format!("item-{index}")).unwrap()
}

fn text_value(prefix: &str, chars: usize) -> String {
    let base = prefix.repeat(chars / prefix.len().max(1) + 1);
    base.chars().take(chars).collect()
}

/// Seeds a running session with attempt 0 completed on node 0 and attempt 1
/// active on node 1, every slot filled at revision 1.
pub(super) fn workchain_session(
    node_items: &[usize],
    snapshot: &ProcedureSnapshotV2,
    session_number: u64,
    value_chars: usize,
) -> GraphSessionStateV2 {
    let node = |index: usize| GraphNodeId::new(format!("n{index}")).unwrap();
    let mut attempts = Vec::new();
    let mut metadata = Vec::new();
    let mut memories = Vec::new();
    for index in 0..2 {
        let active = index == 1;
        attempts.push(
            SessionAttemptV2::new(
                chain_attempt_uuid(session_number, index),
                node(index),
                AttemptNumberV2::FIRST,
                TraceSequenceV2::new((index + 1) as u64),
                if active {
                    AttemptLifecycle::Active
                } else {
                    AttemptLifecycle::Completed
                },
                AttemptValidityV2::Valid,
                None,
            )
            .unwrap(),
        );
        metadata.push(
            AttemptMetadataV2::new(
                chain_attempt_uuid(session_number, index),
                UnixMillis::new(100 + index as u64),
                (!active).then_some(UnixMillis::new(101 + index as u64)),
                (!active).then(|| "advance".to_owned()),
            )
            .unwrap(),
        );
        let slots = (0..node_items[index])
            .map(|item| {
                ItemSlotStateV2::new(
                    chain_attempt_uuid(session_number, index),
                    chain_item_id(item),
                    ItemTypeV1::Text,
                    Revision::new(1),
                    Some(RecordedItemValueV2::text(text_value("a", value_chars)).unwrap()),
                    UnixMillis::new(100 + index as u64),
                    UnixMillis::new(101 + index as u64),
                )
                .unwrap()
            })
            .collect::<Vec<_>>();
        memories.push(
            podway_store::AttemptWorkflowMemoryV2::new(
                chain_attempt_uuid(session_number, index),
                slots,
                Vec::new(),
                Vec::new(),
            )
            .unwrap(),
        );
    }
    let counters = (0..node_items.len())
        .map(|index| GraphNodeCounterV2::new(node(index), u64::from(index < 2), 0))
        .collect::<Vec<_>>();
    GraphSessionStateV2::new_with_workflow_memory(
        Revision::new(2),
        "READS-002 baseline",
        snapshot.clone(),
        SessionTraceV2::from_parts(
            chain_session_id(session_number),
            SessionLifecycle::Running,
            Revision::new(2),
            attempts,
        )
        .unwrap(),
        counters,
        metadata,
        podway_store::WorkflowMemoryStateV2::new(memories, Vec::new(), Vec::new()).unwrap(),
        UnixMillis::new(90),
        None,
        None,
        None,
    )
    .unwrap()
}

pub(super) fn workchain_fixture(
    node_items: &[usize],
    session_number: u64,
    snapshot_number: u64,
    value_chars: usize,
) -> (
    tempfile::TempDir,
    podway_store::direct_store::DirectStoreV1,
    GraphSessionStateV2,
) {
    let identity = DurableWorktreeIdentityV1::new(
        digest('a'),
        WorkspaceId::new(format!("00000000-0000-4000-8000-{session_number:012x}")).unwrap(),
        digest('b'),
    );
    let (root, direct) = fixture(&identity);
    let snapshot = workchain_snapshot(node_items, snapshot_number);
    let state = workchain_session(node_items, &snapshot, session_number, value_chars);
    seed(&direct, &state);
    (root, direct, state)
}

fn replace_with(
    direct: &podway_store::direct_store::DirectStoreV1,
    tag: &str,
    current: &GraphSessionStateV2,
    next: &GraphSessionStateV2,
    now: u64,
) {
    let mutation = request(tag);
    direct
        .mutate(
            &mutation,
            deadline(),
            || UnixMillis::new(now),
            |transaction| {
                transaction.now();
                transaction.replace_graph(
                    current.workspace_revision(),
                    current.trace().revision(),
                    next,
                )?;
                Ok(success(&mutation, json!({"wave": tag}), transaction.now()))
            },
        )
        .unwrap();
}

/// Wave 1/3: a compatible recording mutation on the active attempt — the
/// real `mutate_active_items_v2` domain path, bounded by the 128-item batch
/// limit.
pub(super) fn record_wave(
    direct: &podway_store::direct_store::DirectStoreV1,
    tag: &str,
    current: &GraphSessionStateV2,
    sets: &[(usize, String)],
    now: u64,
) -> GraphSessionStateV2 {
    let active = current
        .trace()
        .active_attempt()
        .unwrap()
        .attempt_id()
        .clone();
    let memory = current
        .workflow_memory()
        .attempts()
        .iter()
        .find(|memory| memory.attempt_id() == &active)
        .unwrap();
    let mutations: Vec<ActiveItemMutationRequestV2> = sets
        .iter()
        .map(|(item, value)| {
            let slot = memory
                .item_slots()
                .iter()
                .find(|slot| slot.item_id() == &chain_item_id(*item))
                .unwrap();
            ActiveItemMutationRequestV2::new(
                chain_item_id(*item),
                slot.revision(),
                ActiveItemMutationV2::Set {
                    value: value.clone(),
                },
            )
        })
        .collect();
    let next = current
        .mutate_active_items_v2(&active, &mutations, UnixMillis::new(now))
        .unwrap()
        .into_state();
    replace_with(direct, tag, current, &next, now);
    next
}

/// Wave 2: a compatible action completion that appends the fresh attempt
/// with revision-ZERO empty mutable memory — the successor rule the
/// approved plan's fixtures rely on.
pub(super) fn append_wave(
    direct: &podway_store::direct_store::DirectStoreV1,
    tag: &str,
    current: &GraphSessionStateV2,
    fresh: AttemptId,
    now: u64,
) -> GraphSessionStateV2 {
    let active = current
        .trace()
        .active_attempt()
        .unwrap()
        .attempt_id()
        .clone();
    let next = current
        .complete_active_action_v2(
            current.trace().revision(),
            &active,
            Some(fresh),
            UnixMillis::new(now),
        )
        .unwrap()
        .into_state();
    replace_with(direct, tag, current, &next, now);
    next
}

/// Sum of the prepared graph's registered payload bytes: the cached-input
/// units participating in the measured revalidation window.
fn prepared_payload_bytes(prepared: &podway_store::direct_store::DirectPreparedGraphV1) -> u64 {
    prepared
        .graph()
        .workflow_memory()
        .attempts()
        .iter()
        .flat_map(|attempt| attempt.item_slots())
        .filter(|slot| slot.value().is_some())
        .map(|slot| {
            payload_bytes_of(slot)
                .map(|bytes| bytes as u64)
                .expect("prepared payload must be registered by its prepare window")
        })
        .sum()
}

fn executes_of(
    report: &podway_store::test_support::query_account::QueryReport,
    needle: &str,
) -> u64 {
    report
        .sql
        .iter()
        .filter(|(key, _)| key.contains(needle))
        .map(|(_, counts)| counts.executes)
        .sum()
}

fn cold_current_graph(
    direct: &podway_store::direct_store::DirectStoreV1,
) -> Result<Option<GraphSessionStateV2>, podway_store::direct_store::DirectStoreErrorV1> {
    direct.read(deadline(), |snapshot| snapshot.current_graph())
}

/// Three-wave 128+128 accumulation: revise all 128 cached active slots,
/// append the fresh attempt EMPTY, then record its 128 slots. The original
/// path used two headers, one cold owner selector and 128 point reloads.
/// The current path asserts one header and ceil(256/128) = two chunks,
/// with unchanged payload retention and pointer reuse. The nonaligned
/// fixture below separately proves packing across owner boundaries.
#[test]
fn reload_baseline_three_waves_128_plus_128() {
    let chain = [128usize, 128, 128];
    let (root, direct, _state) = workchain_fixture(&chain, 140, 240, 24);
    clear_payload_registry();
    let accountant = QueryAccountant::start();
    let prepared = direct.prepare_graph(deadline()).unwrap().unwrap();
    let prepare_report = accountant.finish();
    assert_eq!(prepare_report.retention.fresh_payloads, 256);
    let prepared_bytes = prepared_payload_bytes(&prepared);
    let wave_one_sets: Vec<(usize, String)> =
        (0..128).map(|item| (item, text_value("b", 24))).collect();
    let after_one = record_wave(
        &direct,
        "w1-revise-128",
        &prepared.graph(),
        &wave_one_sets,
        900,
    );
    let after_two = append_wave(
        &direct,
        "w2-append-empty",
        &after_one,
        chain_attempt_uuid(140, 2),
        910,
    );
    let wave_three_sets: Vec<(usize, String)> =
        (0..128).map(|item| (item, text_value("c", 24))).collect();
    let _after_three = record_wave(&direct, "w3-record-128", &after_two, &wave_three_sets, 920);
    let accountant = QueryAccountant::start();
    declare_cached_input_bytes(prepared_bytes);
    let revalidate = request("revalidate-128");
    direct
        .mutate(
            &revalidate,
            deadline(),
            || UnixMillis::new(950),
            |transaction| {
                let current = transaction
                    .use_prepared_graph(Some(&prepared))?
                    .expect("current graph");
                // The appended attempt is fresh; the cached active attempt's
                // slots must not share prepared payload identities.
                let memory_by_attempt = |attempt: usize| {
                    current
                        .workflow_memory()
                        .attempts()
                        .iter()
                        .find(|memory| memory.attempt_id() == &chain_attempt_uuid(140, attempt))
                        .unwrap()
                };
                let prepared_one = prepared
                    .graph()
                    .workflow_memory()
                    .attempts()
                    .iter()
                    .find(|memory| memory.attempt_id() == &chain_attempt_uuid(140, 1))
                    .unwrap();
                for (slot, prepared_slot) in memory_by_attempt(1)
                    .item_slots()
                    .iter()
                    .zip(prepared_one.item_slots())
                {
                    assert_ne!(
                        payload_pointer(slot),
                        payload_pointer(prepared_slot),
                        "a revised slot must not reuse the prepared payload"
                    );
                }
                assert_eq!(memory_by_attempt(2).item_slots().len(), 128);
                Ok(success(
                    &revalidate,
                    json!({"revalidated": true}),
                    transaction.now(),
                ))
            },
        )
        .unwrap();
    let report = accountant.finish();
    assert_mixed_queries(&report, 2);
    assert_eq!(report.retention.fresh_bytes_cumulative, 256 * 50);
    assert_eq!(report.retention.raw_window_peak, 128 * 50);
    assert_eq!(
        report.retention.combined_peak,
        prepared_bytes + 256 * 50 + 50
    );
    assert_eq!(report.retention.reused_payloads, 128);
    // Fresh decodes: 128 reloaded changed payloads plus the appended
    // attempt's 128 cold payloads; unchanged attempt-0 slots are reused.
    assert_eq!(report.retention.fresh_payloads, 256);
    assert_eq!(report.retention.container_copies, 128);
    assert_eq!(report.retention.unknown_reuse_payloads, 0);
    assert_eq!(report.retention.raw_released_at_scope_end, 0);
    println!(
        "READS002_BASELINE|three-waves-128|header_executes=1|chunk_executes=2|point_reloads=0|actual=ceil(256/128)"
    );
    drop(direct);
    drop(root);
}

/// Non-aligned global-packing shape: 2 revised cached slots plus a 4-item
/// fresh node recorded after an empty append — 6 changed/new pairs.
/// One padded chunk spans both owners; per-owner chunking would need two.
#[test]
fn reload_baseline_nonaligned_2_plus_4_global_packing_target() {
    let chain = [2usize, 2, 4];
    let (root, direct, _state) = workchain_fixture(&chain, 141, 241, 24);
    clear_payload_registry();
    let accountant = QueryAccountant::start();
    let prepared = direct.prepare_graph(deadline()).unwrap().unwrap();
    accountant.finish();
    let prepared_bytes = prepared_payload_bytes(&prepared);
    // Pointer identities of every cached slot, keyed by (attempt, item).
    let prepared_pointers: Vec<((usize, usize), Option<usize>)> = [0usize, 1]
        .iter()
        .flat_map(|&attempt| {
            prepared
                .graph()
                .workflow_memory()
                .attempts()
                .iter()
                .find(|memory| memory.attempt_id() == &chain_attempt_uuid(141, attempt))
                .unwrap()
                .item_slots()
                .iter()
                .enumerate()
                .map(move |(item, slot)| ((attempt, item), payload_pointer(slot)))
        })
        .collect();
    let after_one = record_wave(
        &direct,
        "w1-revise-2",
        &prepared.graph(),
        &[(0, text_value("b", 24)), (1, text_value("b", 24))],
        900,
    );
    let after_two = append_wave(
        &direct,
        "w2-append-empty",
        &after_one,
        chain_attempt_uuid(141, 2),
        910,
    );
    let _after_three = record_wave(
        &direct,
        "w3-record-4",
        &after_two,
        &[
            (0, text_value("c", 24)),
            (1, text_value("c", 24)),
            (2, text_value("c", 24)),
            (3, text_value("c", 24)),
        ],
        920,
    );
    let accountant = QueryAccountant::start();
    declare_cached_input_bytes(prepared_bytes);
    let revalidate = request("revalidate-nonaligned");
    direct
        .mutate(
            &revalidate,
            deadline(),
            || UnixMillis::new(950),
            |transaction| {
                let current = transaction
                    .use_prepared_graph(Some(&prepared))?
                    .expect("current graph");
                for ((attempt, item), slot) in [0usize, 1].iter().flat_map(|&attempt| {
                    current
                        .workflow_memory()
                        .attempts()
                        .iter()
                        .find(|memory| memory.attempt_id() == &chain_attempt_uuid(141, attempt))
                        .unwrap()
                        .item_slots()
                        .iter()
                        .enumerate()
                        .map(move |(item, slot)| ((attempt, item), slot))
                }) {
                    let prepared_pointer = prepared_pointers
                        .iter()
                        .find(|((candidate_attempt, candidate_item), _)| {
                            *candidate_attempt == attempt && *candidate_item == item
                        })
                        .unwrap()
                        .1;
                    if attempt == 0 {
                        assert_eq!(
                            payload_pointer(slot),
                            prepared_pointer,
                            "an unchanged cached slot must reuse the prepared payload"
                        );
                    } else {
                        assert_ne!(
                            payload_pointer(slot),
                            prepared_pointer,
                            "a revised cached slot must not reuse the prepared payload"
                        );
                    }
                }
                Ok(success(
                    &revalidate,
                    json!({"revalidated": true}),
                    transaction.now(),
                ))
            },
        )
        .unwrap();
    let report = accountant.finish();
    assert_mixed_queries(&report, 1);
    assert_eq!(report.retention.fresh_bytes_cumulative, 6 * 50);
    assert_eq!(report.retention.raw_window_peak, 4 * 50);
    assert_eq!(report.retention.combined_peak, prepared_bytes + 6 * 50 + 50);
    assert_eq!(report.retention.reused_payloads, 2);
    assert_eq!(report.retention.fresh_payloads, 6);
    assert_eq!(report.retention.container_copies, 2);
    assert_eq!(report.retention.raw_released_at_scope_end, 0);
    println!(
        "READS002_BASELINE|nonaligned-2-plus-4|header_executes=1|chunk_executes=1|point_reloads=0|actual=1_chunk_spans_both_attempts_vs_2_per_attempt"
    );
    drop(direct);
    drop(root);
}

/// Heterogeneous larger-first control: the cached active attempt's changed
/// payloads have encoded lengths (100, 40) in specification order. The old
/// point-reload path's combined peak must equal the matched arithmetic
/// C + max_i(sum_{j<=i} L_j + L_i) = C + 200 exactly, computed from actual
/// encoded lengths — never from raw text-size guesses, and never the
/// homogeneous >= form.
#[test]
fn reload_baseline_heterogeneous_larger_first_exact_combined_peak() {
    let chain = [2usize, 2];
    let (root, direct, _state) = workchain_fixture(&chain, 142, 242, 24);
    clear_payload_registry();
    let accountant = QueryAccountant::start();
    let prepared = direct.prepare_graph(deadline()).unwrap().unwrap();
    accountant.finish();
    let prepared_bytes = prepared_payload_bytes(&prepared);
    // Encoded size of a text value is value.len() + wrapper overhead; the
    // assertion below pins the exact (100, 40) encoded lengths.
    let after_one = record_wave(
        &direct,
        "w1-heterogeneous",
        &prepared.graph(),
        // 74 chars -> encoded 100; 14 chars -> encoded 40 (verified below).
        &[(0, text_value("d", 74)), (1, text_value("e", 14))],
        900,
    );
    let active = after_one
        .workflow_memory()
        .attempts()
        .iter()
        .find(|memory| memory.attempt_id() == &chain_attempt_uuid(142, 1))
        .unwrap();
    let lengths: Vec<u64> = active
        .item_slots()
        .iter()
        .filter_map(|slot| {
            slot.value()
                .map(|value| recorded_value_encoded_size_v2(value).unwrap() as u64)
        })
        .collect();
    assert_eq!(lengths, vec![100, 40], "encoded lengths must be exact");
    let accountant = QueryAccountant::start();
    declare_cached_input_bytes(prepared_bytes);
    let revalidate = request("revalidate-heterogeneous");
    direct
        .mutate(
            &revalidate,
            deadline(),
            || UnixMillis::new(950),
            |transaction| {
                transaction
                    .use_prepared_graph(Some(&prepared))?
                    .expect("current graph");
                Ok(success(
                    &revalidate,
                    json!({"revalidated": true}),
                    transaction.now(),
                ))
            },
        )
        .unwrap();
    let report = accountant.finish();
    assert_mixed_queries(&report, 1);
    assert_eq!(report.retention.fresh_payloads, 2);
    assert_eq!(
        report.retention.fresh_bytes_cumulative,
        lengths.iter().sum::<u64>()
    );
    // The point path holds one raw payload at a time; the decode probe
    // books the in-flight payload twice (raw not yet released, decoded
    // already recorded), so the matched peak is exactly
    // C + max_i(sum_{j<=i} L_j + L_i).
    let expected_peak = prepared_bytes
        + lengths
            .iter()
            .enumerate()
            .map(|(index, _)| lengths[..=index].iter().sum::<u64>() + lengths[index])
            .max()
            .unwrap();
    assert_eq!(expected_peak, prepared_bytes + 200);
    assert_eq!(report.retention.raw_window_peak, 100);
    assert_eq!(report.retention.combined_peak, expected_peak);
    assert_eq!(report.retention.raw_released_at_scope_end, 0);
    println!(
        "READS002_BASELINE|heterogeneous-100-40|cached_input_bytes={prepared_bytes}|combined_peak={expected_peak}|matched=C+200"
    );
    drop(direct);
    drop(root);
}

/// The frozen per-attempt identity-text units remain unchanged; compact
/// production routing records are disclosed separately from payload bytes.
#[test]
fn identity_metadata_units_pin_old_per_attempt_header_retention() {
    let chain = [2usize, 2, 4];
    let (root, direct, _state) = workchain_fixture(&chain, 143, 243, 24);
    // Cold window over the SEEDED fixture (attempts 0-1 only; the appended
    // attempt does not exist yet): every attempt's header row materializes
    // once; the peak is the largest single attempt's identity total,
    // including the map key clone (2 items x (id 6 + type 4 + key 6)).
    let cold = store(root.path());
    let accountant = QueryAccountant::start();
    let loaded = cold_current_graph(&cold).unwrap();
    let report = accountant.finish();
    assert!(loaded.is_some());
    assert_eq!(report.retention.identity_rows, 4);
    assert_eq!(report.retention.identity_window_peak, 32);
    assert_eq!(report.retention.identity_scope_peak, 32);
    assert_eq!(report.retention.identity_released_at_scope_end, 0);
    // Mixed window: the two cached attempts use header statements and the
    // appended attempt uses the cold full-row statement; the identity peak
    // stays one attempt's total (the 4-item appended attempt).
    clear_payload_registry();
    let accountant = QueryAccountant::start();
    let prepared = direct.prepare_graph(deadline()).unwrap().unwrap();
    accountant.finish();
    let prepared_bytes = prepared_payload_bytes(&prepared);
    let after_one = record_wave(
        &direct,
        "w1-revise-2",
        &prepared.graph(),
        &[(0, text_value("b", 24)), (1, text_value("b", 24))],
        900,
    );
    let after_two = append_wave(
        &direct,
        "w2-append-empty",
        &after_one,
        chain_attempt_uuid(143, 2),
        910,
    );
    let _after_three = record_wave(
        &direct,
        "w3-record-4",
        &after_two,
        &[
            (0, text_value("c", 24)),
            (1, text_value("c", 24)),
            (2, text_value("c", 24)),
            (3, text_value("c", 24)),
        ],
        920,
    );
    let accountant = QueryAccountant::start();
    declare_cached_input_bytes(prepared_bytes);
    let revalidate = request("revalidate-metadata");
    direct
        .mutate(
            &revalidate,
            deadline(),
            || UnixMillis::new(950),
            |transaction| {
                transaction
                    .use_prepared_graph(Some(&prepared))?
                    .expect("current graph");
                Ok(success(
                    &revalidate,
                    json!({"revalidated": true}),
                    transaction.now(),
                ))
            },
        )
        .unwrap();
    let report = accountant.finish();
    assert_eq!(report.retention.identity_rows, 8);
    assert_eq!(report.retention.identity_window_peak, 64);
    assert_eq!(report.retention.identity_released_at_scope_end, 0);
    println!(
        "READS002_BASELINE|identity-units|rows={}|window_peak={}|scope_peak={}",
        report.retention.identity_rows,
        report.retention.identity_window_peak,
        report.retention.identity_scope_peak
    );
    drop(direct);
    drop(root);
    let (stamp_size, pair_size) = podway_store::test_support::mixed_routing_type_sizes();
    assert_eq!((stamp_size, pair_size), (32, 16));
    println!("READS002_CANDIDATE|routing-units|stamp={stamp_size}|pair={pair_size}");
}

fn assert_mixed_queries(
    report: &podway_store::test_support::query_account::QueryReport,
    chunks: u64,
) {
    assert_eq!(executes_of(report, "typeof(value_json) != 'null'"), 1);
    assert_eq!(executes_of(report, "value_json IS NOT NULL"), 0);
    assert_eq!(executes_of(report, "attempt_id=?1 AND item_id=?2"), 0);
    let chunk_counts = report
        .sql
        .iter()
        .filter(|(sql, _)| sql.contains("WITH wanted"));
    let (prepares, executes) = chunk_counts.fold((0, 0), |(p, e), (_, counts)| {
        (p + counts.prepares, e + counts.executes)
    });
    assert_eq!((prepares, executes), (chunks, chunks));
}

/// Focused behavior check for the planned acquisition primitives on an
/// isolated scratch database (canonical slot table shape, no production
/// query touched): the fixed 128-slot rowid CTE with 256 identity binds and
/// NULL padding, plus the pinned lazy cursor ownership composite (family
/// advance/get boundary retention + lazy per-chunk statement cells with
/// drop-before-rearm). SQLite behavior here runs on the same pinned bundled
/// toolchain as production.
#[test]
fn rowid_cte_256_binds_and_lazy_cursor_ownership_on_pinned_sqlite() {
    let dir = tempfile::TempDir::new().unwrap();
    let connection = rusqlite::Connection::open(dir.path().join("scratch.db")).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE v2_item_slots (\
               attempt_id TEXT NOT NULL, \
               item_id TEXT NOT NULL, \
               item_type TEXT NOT NULL CHECK (item_type IN ('confirm','text','choice','integer','list','artifact','check_result')), \
               item_revision INTEGER NOT NULL DEFAULT 0 CHECK (item_revision >= 0), \
               value_json TEXT CHECK (value_json IS NULL OR json_valid(value_json)), \
               created_at_ms INTEGER NOT NULL, \
               updated_at_ms INTEGER NOT NULL, \
               PRIMARY KEY (attempt_id, item_id)) STRICT;",
        )
        .unwrap();
    // 130 rows across three attempts; rowids are captured at insert time.
    let mut inserted: Vec<(String, i64)> = Vec::new();
    for attempt in 0..3i64 {
        for item in 0..50i64 {
            let item_id = format!("item-{item:03}");
            let value = format!("{{\"kind\":\"text\",\"value\":\"v{}\"}}", "x".repeat(4));
            connection
                .execute(
                    "INSERT INTO v2_item_slots (attempt_id, item_id, item_type, item_revision, value_json, created_at_ms, updated_at_ms) \
                     VALUES (?1, ?2, 'text', 1, ?3, 10, 11)",
                    rusqlite::params![format!("attempt-{attempt}"), item_id, value],
                )
                .unwrap();
            let rowid: i64 = connection
                .query_row(
                    "SELECT rowid FROM v2_item_slots WHERE attempt_id=?1 AND item_id=?2",
                    rusqlite::params![format!("attempt-{attempt}"), item_id],
                    |row| row.get(0),
                )
                .unwrap();
            inserted.push((format!("attempt-{attempt}"), rowid));
        }
    }
    assert_eq!(inserted.len(), 150);
    // Fixed 128-slot CTE text: literal ordinals, 256 numbered binds. Bind
    // values are owned by a local enum so params_from_iter supplies the
    // exact positional set.
    #[derive(Clone)]
    enum Bind {
        Text(String),
        Int(i64),
        Null,
    }
    impl rusqlite::ToSql for Bind {
        fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
            match self {
                Bind::Text(value) => value.to_sql(),
                Bind::Int(value) => value.to_sql(),
                Bind::Null => rusqlite::types::Null.to_sql(),
            }
        }
    }
    let binds_for = |slice: &[(String, i64)], pad_pairs: usize| -> Vec<Bind> {
        let mut values: Vec<Bind> = slice
            .iter()
            .flat_map(|(attempt, rowid)| [Bind::Text(attempt.clone()), Bind::Int(*rowid)])
            .collect();
        for _ in 0..pad_pairs {
            values.push(Bind::Null);
            values.push(Bind::Null);
        }
        values
    };
    let mut cte = String::from("WITH wanted(ord, attempt_id, rowid) AS (VALUES (0, ?1, ?2)");
    for ord in 1..128u32 {
        cte.push_str(&format!(", ({ord}, ?{}, ?{})", ord * 2 + 1, ord * 2 + 2));
    }
    cte.push_str(") SELECT s.value_json, s.created_at_ms, s.updated_at_ms, s.rowid \
                  FROM wanted w JOIN v2_item_slots s ON s.attempt_id = w.attempt_id AND s.rowid = w.rowid \
                  ORDER BY w.ord");
    // Chunk A: the first 128 real identities.
    let values = binds_for(&inserted[..128], 0);
    assert_eq!(values.len(), 256, "exactly 256 identity binds, zero others");
    let mut rows: Vec<(Option<String>, i64, i64, i64)> = Vec::new();
    {
        let mut statement = connection.prepare(&cte).unwrap();
        let mut cursor = statement
            .query(rusqlite::params_from_iter(values.iter()))
            .unwrap();
        while let Some(row) = cursor.next().unwrap() {
            rows.push((
                row.get::<_, Option<String>>(0).unwrap(),
                row.get::<_, i64>(1).unwrap(),
                row.get::<_, i64>(2).unwrap(),
                row.get::<_, i64>(3).unwrap(),
            ));
        }
    }
    assert_eq!(rows.len(), 128, "every real identity matches exactly once");
    for (index, (_, _, _, rowid)) in rows.iter().enumerate() {
        assert_eq!(
            *rowid, inserted[index].1,
            "delivery order must follow w.ord"
        );
    }
    // Chunk B: the last 22 real identities padded with 106 NULL pairs.
    let values = binds_for(&inserted[128..], 128 - 22);
    assert_eq!(values.len(), 256, "padding keeps the fixed bind count");
    let mut padded: Vec<i64> = Vec::new();
    {
        let mut statement = connection.prepare(&cte).unwrap();
        let mut cursor = statement
            .query(rusqlite::params_from_iter(values.iter()))
            .unwrap();
        while let Some(row) = cursor.next().unwrap() {
            padded.push(row.get::<_, i64>(3).unwrap());
        }
    }
    assert_eq!(padded.len(), 22, "NULL padding never matches");
    let expected_tail: Vec<i64> = inserted.iter().skip(128).map(|(_, rowid)| *rowid).collect();
    assert_eq!(padded, expected_tail);
    // A missing identity simply does not deliver: deleting one row makes
    // the delivered sequence shorter than the requested identity list, the
    // fail-closed count/rowid check the plan relies on.
    connection
        .execute("DELETE FROM v2_item_slots WHERE rowid=?1", [inserted[0].1])
        .unwrap();

    // Lazy cursor ownership composite (mirrors the pinned MRE shapes with
    // the real error type): a family cursor with an owned pending key and
    // advance/get boundary retention, plus lazy per-chunk statement cells
    // re-armed through drop(rows.take()).
    use podway_store::{StoreErrorV1, StoreRecordKindV1};
    use rusqlite::fallible_streaming_iterator::FallibleStreamingIterator;
    use std::cell::LazyCell;
    fn shape_error(error: rusqlite::Error) -> StoreErrorV1 {
        // Shape-only classification mirror; production maps through
        // map_rusqlite_error_v1, which these scratch queries never reach.
        let _ = error;
        StoreErrorV1::CorruptStateV1 {
            record: StoreRecordKindV1::Item,
        }
    }
    struct Cursor<'stmt> {
        rows: rusqlite::Rows<'stmt>,
        pending: Option<String>,
        done: bool,
    }
    impl Cursor<'_> {
        fn group(&mut self, key: &str) -> rusqlite::Result<Vec<i64>> {
            let mut out = Vec::new();
            loop {
                if self.done {
                    return Ok(out);
                }
                if self.pending.is_none() {
                    self.rows.advance()?;
                }
                let Some(row) = self.rows.get() else {
                    self.done = true;
                    return Ok(out);
                };
                let row_key: String = row.get(0)?;
                if row_key != key {
                    self.pending = Some(row_key);
                    return Ok(out);
                }
                out.push(row.get::<_, i64>(1)?);
                self.pending = None;
            }
        }
    }
    let mut family_statement = LazyCell::new(|| {
        connection
            .prepare("SELECT attempt_id, rowid FROM v2_item_slots ORDER BY attempt_id, item_id")
            .map_err(shape_error)
    });
    let mut family_rows = LazyCell::new(|| {
        LazyCell::force_mut(&mut family_statement)
            .as_mut()
            .map_err(|error| error.clone())?
            .query([])
            .map(|rows| Cursor {
                rows,
                pending: None,
                done: false,
            })
            .map_err(shape_error)
    });
    let chunk_count = 150usize.div_ceil(128);
    let mut chunk_statements = (0..chunk_count)
        .map(|_| LazyCell::new(|| connection.prepare(&cte).map_err(shape_error)))
        .collect::<Vec<_>>();
    let mut chunk_iter = chunk_statements.iter_mut();
    let mut chunk_rows: Option<rusqlite::Rows<'_>> = None;
    let mut delivered_rowids: Vec<i64> = Vec::new();
    for (index, (attempt, _rowid)) in inserted.iter().enumerate() {
        if index % 128 == 0 {
            drop(chunk_rows.take());
            let cell = chunk_iter.next().expect("chunk statement cell");
            let statement = LazyCell::force_mut(cell)
                .as_mut()
                .map_err(|error| error.clone())
                .unwrap();
            let base = index.min(128);
            let remaining = (inserted.len() - base).min(128);
            let values = binds_for(&inserted[base..base + remaining], 128 - remaining);
            chunk_rows = Some(
                statement
                    .query(rusqlite::params_from_iter(values.iter()))
                    .unwrap(),
            );
        }
        // Scalar-key boundary retention: the family cursor holds the next
        // group's key without converting any of its payload columns.
        let group = LazyCell::force_mut(&mut family_rows)
            .as_mut()
            .map_err(|error| error.clone())
            .unwrap()
            .group(attempt)
            .unwrap();
        if !group.is_empty() {
            // Only the group owning the deleted row is short by one.
            assert_eq!(group.len(), if index == 0 { 49 } else { 50 });
        }
        if let Some(row) = chunk_rows.as_mut().unwrap().next().unwrap() {
            delivered_rowids.push(row.get::<_, i64>(3).unwrap());
        }
    }
    let deleted_rowid = inserted[0].1;
    let expected: Vec<i64> = inserted
        .iter()
        .filter(|(_, rowid)| *rowid != deleted_rowid)
        .map(|(_, rowid)| *rowid)
        .collect();
    assert_eq!(
        delivered_rowids.len(),
        149,
        "the deleted identity is simply absent"
    );
    assert_eq!(
        delivered_rowids, expected,
        "delivery follows identity order"
    );
    drop(chunk_rows.take());
    drop(dir);
}

/// Inspect SQL actually executed by production, on the configured native
/// Store connection. In particular cold payloads must not be repacked into
/// a larger sorter/automatic-index record near SQLITE_LIMIT_LENGTH.
#[test]
fn cold_bulk_query_shapes_use_the_real_snapshot_connection() {
    use super::int_reads001_query_inventory::{reads001_fixture, recipe_small};
    let (_root, direct, _state) = reads001_fixture(&recipe_small(), 180, 280);
    let before = super::int_v2dfc008_persistence::image(&direct);
    let shm_before = direct.layout().shm().exists();
    let accountant = QueryAccountant::start();
    direct
        .read(deadline(), |snapshot| snapshot.current_graph())
        .unwrap()
        .unwrap();
    let report = accountant.finish();
    let families = [
        "value_json IS NOT NULL",
        "FROM v2_blockers",
        "FROM v2_resolved_evidence_references",
        "criterion_ordinal, statement FROM v2_goal_criteria",
        "citation_ordinal, citation_kind",
    ];
    for (index, needle) in families.into_iter().enumerate() {
        let sql = report.sql.keys().find(|sql| sql.contains(needle)).unwrap();
        let plan = direct
            .read(deadline(), |snapshot| {
                podway_store::test_support::explain_read_query(snapshot, sql)
            })
            .unwrap();
        assert!(!plan.is_empty());
        if index == 0 {
            assert!(
                plan.iter().all(|op| {
                    !op.opcode.starts_with("Sorter")
                        && !matches!(
                            op.opcode.as_str(),
                            "MakeRecord" | "OpenAutoindex" | "OpenEphemeral"
                        )
                }),
                "cold slot rows must pass through without repacking: {plan:?}"
            );
        }
        let repacking: Vec<_> = plan
            .iter()
            .filter(|op| {
                op.opcode.starts_with("Sorter")
                    || matches!(
                        op.opcode.as_str(),
                        "MakeRecord" | "OpenAutoindex" | "OpenEphemeral"
                    )
            })
            .collect();
        println!(
            "READS002_NATIVE_PLAN|family={index}|sqlite={}|repacking={repacking:?}",
            rusqlite::version()
        );
    }
    assert_eq!(direct.layout().shm().exists(), shm_before);
    assert_eq!(super::int_v2dfc008_persistence::image(&direct), before);
}

/// Declaration order differs from item_id ASC on the appended node: the
/// NEW owner's collection replay stays row-major (item_id ASC) while its
/// spec validation follows the reversed declaration, and the mixed path
/// stays healthy with the registered counts.
#[test]
fn reload_baseline_new_owner_with_nonlexicographic_declaration_order() {
    let chain = [2usize, 2, 2];
    let identity = DurableWorktreeIdentityV1::new(
        digest('a'),
        WorkspaceId::new("00000000-0000-4000-8000-000000000147").unwrap(),
        digest('b'),
    );
    let (root, direct) = fixture(&identity);
    // Node n2 (the fresh owner's node) declares its items in reverse id
    // order; rows still sort by item_id ASC in the database.
    let snapshot = workchain_snapshot_reversed_last(&chain, 247);
    // The declaration order really differs from item_id ASC.
    let declared: Vec<String> = snapshot
        .item_declarations_v2(&GraphNodeId::new("n2").unwrap())
        .unwrap()
        .iter()
        .map(|item| item.id().as_str().to_owned())
        .collect();
    let mut sorted = declared.clone();
    sorted.sort();
    assert_ne!(declared, sorted, "n2 declarations must be nonlexicographic");
    let state = workchain_session(&chain, &snapshot, 147, 24);
    seed(&direct, &state);
    clear_payload_registry();
    let accountant = QueryAccountant::start();
    let prepared = direct.prepare_graph(deadline()).unwrap().unwrap();
    accountant.finish();
    let prepared_bytes = prepared_payload_bytes(&prepared);
    let after_revise = record_wave(
        &direct,
        "w0-revise-2",
        &prepared.graph(),
        &[(0, text_value("b", 24)), (1, text_value("b", 24))],
        890,
    );
    let after_append = append_wave(
        &direct,
        "w1-append-empty",
        &after_revise,
        chain_attempt_uuid(147, 2),
        900,
    );
    let _after_record = record_wave(
        &direct,
        "w2-record-2",
        &after_append,
        &[(0, text_value("c", 24)), (1, text_value("c", 24))],
        910,
    );
    let accountant = QueryAccountant::start();
    declare_cached_input_bytes(prepared_bytes);
    let revalidate = request("revalidate-nonlex");
    direct
        .mutate(
            &revalidate,
            deadline(),
            || UnixMillis::new(950),
            |transaction| {
                let current = transaction
                    .use_prepared_graph(Some(&prepared))?
                    .expect("current graph");
                let new_memory = current
                    .workflow_memory()
                    .attempts()
                    .iter()
                    .find(|memory| memory.attempt_id() == &chain_attempt_uuid(147, 2))
                    .expect("appended owner");
                // The NEW owner validated the reversed declaration
                // row-major while every declared slot resolved.
                assert_eq!(new_memory.item_slots().len(), 2);
                Ok(success(
                    &revalidate,
                    json!({"revalidated": true}),
                    transaction.now(),
                ))
            },
        )
        .unwrap();
    let report = accountant.finish();
    assert_eq!(executes_of(&report, "typeof(value_json) != 'null'"), 1);
    let chunk_executes: u64 = report
        .sql
        .iter()
        .filter(|(key, _)| key.contains("WITH wanted"))
        .map(|(_, counts)| counts.executes)
        .sum();
    assert_eq!(chunk_executes, 1);
    assert_eq!(executes_of(&report, "attempt_id=?1 AND item_id=?2"), 0);
    assert_eq!(report.retention.raw_released_at_scope_end, 0);
    println!(
        "READS002_BASELINE|nonlex-declaration|header_executes=1|chunk_executes=1|point_reloads=0|healthy"
    );
    drop(direct);
    drop(root);
}

/// Deferred discovery scratch ownership: a future failing owner's raw
/// header bytes stay booked across earlier owners' slot-scope exits, the
/// combined identity window peak observes them, and the unwind books what
/// the fault row never consumed.
#[test]
fn deferred_scratch_survives_earlier_scope_exits_and_books_unwind() {
    let chain = [2usize, 2, 2];
    let identity = DurableWorktreeIdentityV1::new(
        digest('a'),
        WorkspaceId::new("00000000-0000-4000-8000-000000000148").unwrap(),
        digest('b'),
    );
    let (root, direct) = fixture(&identity);
    let snapshot = workchain_snapshot(&chain, 248);
    let state = workchain_session(&chain, &snapshot, 148, 24);
    seed(&direct, &state);
    clear_payload_registry();
    let accountant = QueryAccountant::start();
    let prepared = direct.prepare_graph(deadline()).unwrap().unwrap();
    accountant.finish();
    // Revise the cached active attempt's item so its revision differs from
    // the prepared graph; append the third owner and record its first
    // item (the valid prefix the future owner retains).
    let after_revise = record_wave(
        &direct,
        "w0-revise",
        &prepared.graph(),
        &[(0, text_value("b", 24))],
        890,
    );
    let after_append = append_wave(
        &direct,
        "w1-append",
        &after_revise,
        chain_attempt_uuid(148, 2),
        900,
    );
    let _after_record = record_wave(
        &direct,
        "w2-record",
        &after_append,
        &[(0, text_value("c", 24))],
        910,
    );
    // Injection 1: the cached attempt's changed row becomes noncanonical
    // JSON with its revision advanced, so the unchanged-stamp reuse cannot
    // bypass it — the failure fires at its spec-position decode. Injection
    // 2: the NEW owner's second row id becomes invalid UTF-8; discovery
    // stops at that row and retains the owner's staged valid prefix.
    let (changed_value, changed_id);
    {
        let connection = rusqlite::Connection::open(direct.layout().database()).unwrap();
        changed_value = connection
            .execute(
                "UPDATE v2_item_slots SET value_json=?2, item_revision=item_revision+1 \
                 WHERE attempt_id=?1 AND item_id='item-0'",
                rusqlite::params![
                    chain_attempt_uuid(148, 1).as_str(),
                    "{\"value\":\"x\",\"kind\":\"text\"}",
                ],
            )
            .unwrap();
        changed_id = connection
            .execute(
                "UPDATE v2_item_slots SET item_id=CAST(x'ff' AS TEXT) \
                 WHERE attempt_id=?1 AND item_id='item-1'",
                [chain_attempt_uuid(148, 2).as_str()],
            )
            .unwrap();
    }
    assert_eq!(changed_value, 1, "exactly the cached changed row corrupts");
    assert_eq!(
        changed_id, 1,
        "exactly the future owner's second row corrupts"
    );
    let after_injection_main_db = std::fs::read(direct.layout().database()).unwrap();
    let after_injection_wal = std::fs::read(direct.layout().wal()).ok();
    let accountant = QueryAccountant::start();
    let revalidate = request("revalidate-scratch");
    let refused = direct.mutate(
        &revalidate,
        deadline(),
        || UnixMillis::new(950),
        |transaction| {
            transaction.use_prepared_graph(Some(&prepared))?;
            Ok(success(
                &revalidate,
                json!({"revalidated": true}),
                transaction.now(),
            ))
        },
    );
    let report = accountant.finish();
    // Typed winner: the cached owner's changed payload decode fails as an
    // Item corruption at its spec position, before the future owner's
    // replay.
    assert!(matches!(
        &refused,
        Err(podway_store::direct_store::DirectStoreErrorV1::State(
            podway_store::StoreErrorV1::CorruptStateV1 {
                record: podway_store::StoreRecordKindV1::Item,
            }
        ))
    ));
    // The corrupt payload reached the decoder exactly once (owner stage
    // evidence, not just is_err).
    assert_eq!(report.retention.decode_attempts, 1);
    assert_eq!(report.retention.fresh_payloads, 0);
    // Attempt 0 fully validated (its slot scope exited) while the NEW
    // failing owner's discovery scratch stayed booked; the combined
    // identity window peak is the exact sum of the live scope identity
    // and the retained future scratch, not merely a weak inequality.
    // The scratch peak is the transient two-row staging of any fully
    // valid owner before its release; the retained balance is the failing
    // owner's valid prefix alone.
    let scratch_retained = 6 + 4; // item-0's retained id+type bytes
    let staging_transient = 2 * (6 + 4); // one owner's two rows staged at once
    let scope_identity = 2 * (6 + 4) + 2 * 6; // one cached attempt's two materialized rows
    assert_eq!(report.retention.discovery_scratch_peak, staging_transient);
    assert_eq!(
        report.retention.identity_window_peak,
        (scope_identity + scratch_retained) as u64,
        "window peak must be the sum of live scope identity and retained scratch"
    );
    // The corrupt decode stopped the load before the future owner's
    // replay, so its never-consumed scratch was booked at unwind.
    assert_eq!(
        report.retention.discovery_scratch_released_at_abort,
        scratch_retained as u64
    );
    assert_eq!(report.retention.discovery_scratch_current, 0);
    // Canonical outcome contract: Podway records validated success
    // outcomes only, atomically with their state; every rejection —
    // including this one — leaves no outcome row and no state change,
    // and a rejected read-only callback appends no WAL frames (the
    // store's callback-error path returns before success validation,
    // persistence and checkpoint). A read-write open may create WAL/SHM
    // bookkeeping, so persistence evidence here is: (a) the supported
    // outcome lookup is empty for the rejected key, (b) byte-identical
    // database and preexisting WAL bytes (a fresh WAL may carry only an
    // empty header, never frames), (c) SHM presence is preserved across
    // the read-only refusal, and (d) the fresh read-only open reproduces
    // the identical refusal with no rejection memoization.
    drop(direct);
    let unobserved_store = store(root.path());
    let shm_before = unobserved_store.layout().shm().exists();
    let unobserved = unobserved_store.read(deadline(), |snapshot| {
        assert!(
            snapshot.outcome(revalidate.key())?.is_none(),
            "a rejected mutation records no outcome"
        );
        snapshot.current_graph()
    });
    assert_eq!(
        format!("{:?}", refused.err()),
        format!("{:?}", unobserved.err())
    );
    assert_eq!(
        unobserved_store.layout().shm().exists(),
        shm_before,
        "the read-only refusal neither creates nor removes the sidecar"
    );
    let main_db_after = std::fs::read(unobserved_store.layout().database()).unwrap();
    assert_eq!(
        main_db_after, after_injection_main_db,
        "a rejected read-only callback leaves the database bytes unchanged"
    );
    let after_wal = std::fs::read(unobserved_store.layout().wal()).ok();
    match (&after_injection_wal, &after_wal) {
        (Some(before), Some(after)) => assert_eq!(
            after, before,
            "a rejected read-only callback appends no WAL frames"
        ),
        (None, Some(after)) => assert!(
            after.is_empty() || after.len() == 32,
            "a fresh WAL may carry only an empty header, not frames"
        ),
        (None, None) => {}
        (Some(_), None) => panic!("rejection must preserve an existing WAL"),
    }
    drop(root);
}

/// Production chunk statement plan on the real configured snapshot
/// connection: metadata-only record assembly (one MakeRecord of four
/// fields), rowid seek before payload columns, no automatic payload
/// index, and the fixed 256 identity parameters.
#[test]
fn production_chunk_sql_plan_is_metadata_only_on_real_connection() {
    let chain = [2usize, 2];
    let (root, direct, _state) = workchain_fixture(&chain, 149, 249, 24);
    let cold = store(root.path());
    cold.read(deadline(), |snapshot| {
        let opcodes = podway_store::test_support::explain_read_query(
            snapshot,
            podway_store::test_support::mixed_chunk_sql(),
        )
        .unwrap();
        let records: Vec<&podway_store::test_support::ReadQueryOpcode> = opcodes
            .iter()
            .filter(|opcode| opcode.opcode == "MakeRecord")
            .collect();
        assert_eq!(records.len(), 1, "exactly one record assembly");
        assert_eq!(
            records[0].p2, 4,
            "the sorted record is metadata-only (ord, id, rowid, sequence)"
        );
        assert!(
            opcodes.iter().any(|opcode| opcode.opcode == "SeekRowid"),
            "identity seek precedes payload columns"
        );
        assert!(
            !opcodes
                .iter()
                .any(|opcode| opcode.opcode.contains("Autoindex")),
            "no automatic payload index"
        );
        let seek = opcodes
            .iter()
            .position(|op| op.opcode == "SeekRowid")
            .unwrap();
        let record = opcodes
            .iter()
            .position(|op| op.opcode == "MakeRecord")
            .unwrap();
        let sort = opcodes.iter().position(|op| op.opcode == "Sort").unwrap();
        assert!(
            record < sort && sort < seek,
            "sort metadata before seeking payload"
        );
        let payload_cursor = opcodes[seek].p1;
        for column in [4, 5, 6] {
            let position = opcodes
                .iter()
                .position(|op| op.opcode == "Column" && op.p1 == payload_cursor && op.p2 == column)
                .unwrap();
            assert!(position > seek, "payload columns follow rowid seek");
        }
        assert!(
            opcodes.iter().all(|op| !op.opcode.starts_with("Sorter")),
            "no payload sorter: {opcodes:?}"
        );
        assert_eq!(rusqlite::version(), "3.51.3");
        assert_eq!(
            podway_store::test_support::read_query_settings(
                snapshot,
                podway_store::test_support::mixed_chunk_sql()
            )?,
            (256, true, 1_064_960)
        );
        Ok(())
    })
    .unwrap();
    // Actual production routing sizes replace the checkpoint mirrors.
    let (stamp_size, pair_size) = podway_store::test_support::mixed_routing_type_sizes();
    println!(
        "READS002_BASELINE|production-units|row_stamp={stamp_size}|pair_entry={pair_size}|chunk_binds=256"
    );
    drop(direct);
    drop(root);
}
