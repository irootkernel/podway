//! Retained runtime domain assertions at the typed direct boundary and real native Store.
use super::{
    int_v2dfc006_direct_execution::{Clock, deadline, digest, fixture},
    int_v2dfc006_fixture_ports::{ACTION_READBACK_PROCEDURE, STATE_PROCEDURE},
    int_v2dfc006_runtime_ports::{Engine, NoClock},
};
use podway_core::{
    AttemptId, CanonicalProcedureJsonV1, ItemId, ProcedureSnapshotId, ProcedureSourceLabelV1,
    Revision, SessionId, SessionLifecycle, SessionTraceV2, Sha256Digest, UnixMillis,
    canonicalize_json_v1,
};
use podway_engine::{
    direct_api::{DirectCommandV1 as D, DirectResponseV1},
    direct_execution::{DirectCursorCommandV1 as C, DirectReadCommandV1 as R},
    direct_items::DirectItemCommandV1 as I,
    direct_sessions::DirectSessionCommandV1 as S,
    direct_start::DirectStartCommandV1 as T,
    execution::{ExecutionClockV1, prepare_workspace_procedure_source_v2},
    v2_read_service::GraphStatusTierV2,
    workspace::DirectWorkspaceResolverV1,
};
use podway_git::NativeGitResolverV1;
use podway_protocol::*;
use podway_store::{
    CanonicalExecutionJsonV1, GraphNodeCounterV2, GraphSessionStateV2, IdempotencyKeyV1,
    WorkflowMemoryStateV2,
    direct_store::{DirectMutationRequestV1, DirectOutcomeScopeV1},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

#[test]
fn bound003_source_and_stored_adapters_share_runtime_declarations() {
    fn compare(source: podway_engine::execution::PreparedProcedureSourceV2) {
        let expected_digest = source.digest().clone();
        let snapshot = source
            .into_snapshot(
                ProcedureSnapshotId::new("00000000-0000-4000-8000-000000000030").unwrap(),
                Clock.now(),
            )
            .unwrap();
        let stored = podway_store::admit_procedure_snapshot_v2(
            snapshot.snapshot_id().clone(),
            snapshot.canonical_json().clone(),
            expected_digest.clone(),
            snapshot.source().clone(),
            snapshot.created_at(),
        )
        .unwrap();
        assert_eq!(snapshot.digest(), &expected_digest);
        // Compares metadata, topology, all item/evidence/decision declarations and the
        // retained model result, without regenerating stored canonical identity.
        assert_eq!(snapshot.runtime(), stored.runtime());
        for node in snapshot.graph_nodes() {
            assert_eq!(
                snapshot.item_declarations_v2(node.graph_node_id()).unwrap(),
                stored.item_declarations_v2(node.graph_node_id()).unwrap()
            );
        }
    }
    for preset in podway_presets::catalog_v2().list() {
        let source =
            podway_engine::execution::prepare_preset_procedure_source_v2(preset.metadata.id)
                .unwrap()
                .unwrap();
        assert_eq!(source.digest().as_str(), preset.shipped_digest);
        compare(source);
    }
    for (path, bytes) in [
        (
            "equivalent.json",
            include_bytes!("../../../tests/fixtures/v2/procedures/equivalent-procedure.json")
                .as_slice(),
        ),
        (
            "equivalent.yaml",
            include_bytes!("../../../tests/fixtures/v2/procedures/equivalent-procedure.yaml")
                .as_slice(),
        ),
    ] {
        compare(prepare_workspace_procedure_source_v2(path, bytes).unwrap());
    }
}

#[test]
fn bound003_runtime_defaults_preserve_unmaterialized_snapshot_identity() {
    let document = json!({
        "schema":"podway.procedure/v2", "id":"historical-default", "version":"1",
        "name":"Historical default", "purpose":"Preserve stored identity.",
        "node_definitions":{"work":{"type":"action", "title":"Work", "intent":"Record.",
            "items":[{"id":"values", "type":"list", "prompt":"Values", "required":false}]}},
        "graph":{"entry":"work", "nodes":[{"id":"work", "use":"work", "terminal":true}]}
    });
    let canonical = canonicalize_json_v1(&document).unwrap();
    let digest =
        Sha256Digest::new(format!("sha256:{:x}", Sha256::digest(canonical.as_bytes()))).unwrap();
    let stored = podway_store::admit_procedure_snapshot_v2(
        ProcedureSnapshotId::new("00000000-0000-4000-8000-000000000031").unwrap(),
        CanonicalProcedureJsonV1::new(canonical.clone()).unwrap(),
        digest.clone(),
        ProcedureSourceLabelV1::file("historical.json").unwrap(),
        Clock.now(),
    )
    .unwrap();
    let fresh = prepare_workspace_procedure_source_v2("fresh.json", canonical.as_bytes())
        .unwrap()
        .into_snapshot(stored.snapshot_id().clone(), Clock.now())
        .unwrap();
    assert_eq!(stored.canonical_json().as_str(), canonical);
    assert_eq!(stored.digest(), &digest);
    assert_ne!(fresh.digest(), stored.digest());
    assert_eq!(
        fresh
            .item_declarations_v2(fresh.entry_graph_node_id())
            .unwrap(),
        stored
            .item_declarations_v2(stored.entry_graph_node_id())
            .unwrap()
    );
}

#[test]
fn bound001_deferred_memory_error_keeps_public_mapping_and_fence_priority() {
    let engine = empty(STATE_PROCEDURE);
    let canonical = canonicalize_json_v1(&json!({
        "schema":"podway.procedure/v2", "id":"deferred-memory", "version":"1",
        "name":"Deferred memory", "purpose":"Preserve persisted admission timing.",
        "node_definitions":{"work":{"type":"action", "title":"Work", "intent":"Record.",
            "items":[{"id":"proof", "type":"text", "prompt":"Proof", "required":true,
                "min_length":2, "max_length":1}]}},
        "graph":{"entry":"work", "nodes":[{"id":"work", "use":"work", "terminal":true}]}
    }))
    .unwrap();
    let snapshot = podway_store::admit_procedure_snapshot_v2(
        ProcedureSnapshotId::new("00000000-0000-4000-8000-000000000010").unwrap(),
        CanonicalProcedureJsonV1::new(canonical.clone()).unwrap(),
        Sha256Digest::new(format!("sha256:{:x}", Sha256::digest(canonical.as_bytes()))).unwrap(),
        ProcedureSourceLabelV1::file("persisted.json").unwrap(),
        Clock.now(),
    )
    .unwrap();
    let counters = snapshot
        .graph_nodes()
        .iter()
        .map(|node| GraphNodeCounterV2::new(node.graph_node_id().clone(), 0, 0))
        .collect();
    let graph = GraphSessionStateV2::new_with_workflow_memory(
        Revision::new(1),
        "Deferred memory",
        snapshot,
        SessionTraceV2::prepared(SessionId::new("00000000-0000-4000-8000-000000000011").unwrap()),
        counters,
        vec![],
        WorkflowMemoryStateV2::new(vec![], vec![], vec![]).unwrap(),
        Clock.now(),
        None,
        None,
        None,
    )
    .unwrap();
    let seed = DirectMutationRequestV1::new(
        IdempotencyKeyV1::new("deferred-seed").unwrap(),
        digest(),
        "workspace.repair",
        "00000000-0000-4000-8000-000000000002",
        DirectOutcomeScopeV1::Workspace,
    )
    .unwrap();
    engine
        .store
        .mutate(
            &seed,
            deadline(),
            || Clock.now(),
            |transaction| {
                transaction.now();
                transaction.create_graph(&graph)?;
                Ok(CanonicalExecutionJsonV1::new(
                    canonicalize_json_v1(&json!({
                        "schema":"podway.output/v4", "request_id":seed.response_id(),
                        "command":seed.command(), "generated_at":"2023-11-14T22:13:20.000Z",
                        "result":{"seeded":true}, "warnings":[]
                    }))
                    .unwrap(),
                )
                .unwrap())
            },
        )
        .unwrap();
    let command = |revision| {
        D::Procedure(ProcedureV2MutationCommandV1::SessionBegin(SessionBeginV1 {
            initial_goal: None,
            preconditions: SessionIdentityPreconditionsWireV1 {
                expected_session_id: graph.trace().session_id().clone(),
                expected_session_revision: revision,
            },
        }))
    };
    let context = podway_engine::direct_error::DirectErrorContextV1 {
        request_id: RequestIdV1::new("00000000-0000-4000-8000-000000000099").unwrap(),
        command: CommandNameV1::new("session.begin").unwrap(),
        observed_at: Rfc3339MillisV1::new("2023-11-14T22:13:20.000Z").unwrap(),
        idempotency_key: Some(IdempotencyKeyV1::new("deferred-invalid").unwrap()),
        timeout_ms: 5000,
        maintenance: None,
    };
    let render = |revision| {
        let failure = graph
            .begin_v2(
                revision,
                AttemptId::new("00000000-0000-4000-8000-000000000012").unwrap(),
                None,
                Clock.now(),
            )
            .unwrap_err();
        podway_engine::direct_error::render_direct_error_v1(
            &podway_engine::direct_execution::DirectExecutionErrorV1::Store(failure.into()),
            &context,
        )
        .unwrap()
        .into_value()
    };
    let stale = render(Revision::new(1));
    assert_eq!(stale["code"], "SESSION_REVISION_CONFLICT");
    assert_eq!(stale["exit_code"], 4);
    assert_eq!(stale["retryable"], true);
    assert_eq!(stale["details"]["expected_revision"], 1);
    assert_eq!(stale["details"]["current_revision"], 0);
    let invalid = render(Revision::ZERO);
    assert_eq!(invalid["code"], "INTERNAL_ERROR");
    assert_eq!(invalid["exit_code"], 6);
    assert_eq!(invalid["retryable"], false);
    assert_eq!(
        invalid["details"],
        json!({
            "schema":"podway.internal-error-details/v2", "diagnostic_id":context.request_id,
        })
    );
    let sequence = engine.sequence();
    for revision in [Revision::ZERO, Revision::new(1)] {
        // Cold persisted admission rejects before any domain revision fence.
        let DirectResponseV1::Failure(error) =
            engine.execute(engine.request(command(revision), "deferred-cold"), &NoClock)
        else {
            panic!("malformed persisted declaration was admitted")
        };
        let error = error.into_value();
        assert_eq!(error["code"], "WORKSPACE_STATE_UNREADABLE");
        assert_eq!(error["exit_code"], 5);
        assert_eq!(error["retryable"], false);
        assert_eq!(
            error["details"]["schema"],
            "podway.workspace-recovery-details/v2"
        );
        assert_eq!(engine.sequence(), sequence);
        assert!(
            engine
                .store
                .read(deadline(), |snapshot| snapshot
                    .outcome(&IdempotencyKeyV1::new("deferred-cold").unwrap(),))
                .unwrap()
                .is_none()
        );
    }
}

fn empty(source: &str) -> Engine {
    let (temporary, selector, store, _) = fixture();
    std::fs::write(temporary.path().join("source.yaml"), source).unwrap();
    Engine {
        temporary,
        selector,
        store,
        resolver: DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest()),
    }
}
fn start(engine: &Engine, dry_run: bool) -> SessionStartV1 {
    let source = std::fs::read(engine.temporary.path().join("source.yaml")).unwrap();
    SessionStartV1 {
        source: SessionStartSourceV1::Procedure {
            procedure: "source.yaml".into(),
        },
        expected_procedure_digest: Some(
            prepare_workspace_procedure_source_v2("source.yaml", &source)
                .unwrap()
                .digest()
                .clone(),
        ),
        task_title: "Retained runtime domain".into(),
        dry_run,
    }
}
fn prepared(source: &str) -> Engine {
    let engine = empty(source);
    engine.success(D::Start(T::Start(start(&engine, false))), "start");
    engine
}
fn identity(engine: &Engine) -> SessionIdentityPreconditionsWireV1 {
    let graph = engine.graph();
    SessionIdentityPreconditionsWireV1 {
        expected_session_id: graph.trace().session_id().clone(),
        expected_session_revision: graph.trace().revision(),
    }
}
fn begin(engine: &Engine, key: &str) -> Value {
    engine.success(
        D::Procedure(ProcedureV2MutationCommandV1::SessionBegin(SessionBeginV1 {
            initial_goal: None,
            preconditions: identity(engine),
        })),
        key,
    )
}
fn status(engine: &Engine) -> Value {
    engine.read(R::Status {
        tier: GraphStatusTierV2::Verbose,
        history_before: None,
    })
}
fn current(engine: &Engine) -> Option<GraphSessionStateV2> {
    engine
        .store
        .read(deadline(), |snapshot| snapshot.current_graph())
        .unwrap()
}
fn reject(engine: &Engine, command: D, key: &str, code: &str) -> Value {
    let before = current(engine);
    let sequence = engine.sequence();
    let prior = engine
        .store
        .read(deadline(), |snapshot| {
            snapshot.outcome(&IdempotencyKeyV1::new(key).unwrap())
        })
        .unwrap();
    let DirectResponseV1::Failure(error) = engine.execute(engine.request(command, key), &Clock)
    else {
        panic!("expected {code}")
    };
    let error = error.into_value();
    assert_eq!(error["code"], code);
    assert_eq!(current(engine), before);
    assert_eq!(engine.sequence(), sequence);
    assert_eq!(
        engine
            .store
            .read(deadline(), |snapshot| snapshot
                .outcome(&IdempotencyKeyV1::new(key).unwrap()))
            .unwrap(),
        prior
    );
    error
}
fn reset(engine: &Engine, mode: SessionDeletionModeV1, dry_run: bool) -> D {
    D::Session(S::Reset(SessionResetV1 {
        mode,
        dry_run,
        preconditions: identity(engine),
    }))
}
fn cancel(engine: &Engine, key: &str) -> Value {
    engine.success(
        D::Cursor(C::Cancel(SessionCancelV1 {
            reason: "No longer needed".into(),
            preconditions: engine.fences(),
        })),
        key,
    )
}
fn disposition(engine: &Engine) -> D {
    D::Procedure(ProcedureV2MutationCommandV1::SessionTerminalDisposition(
        SessionTerminalDispositionV1 {
            disposition: TerminalDispositionInputV1::NotRequired {
                reason: "The isolated session needs no external handoff.".into(),
            },
            actor: None,
            preconditions: identity(engine),
        },
    ))
}
fn replace(engine: &Engine, mode: SessionDeletionModeV1, dry_run: bool) -> D {
    D::Start(T::Replace(SessionStartReplaceV1 {
        start: start(engine, dry_run),
        mode,
        preconditions: identity(engine),
    }))
}
fn archives(engine: &Engine) -> Value {
    engine.success(
        D::Read {
            command: R::ArchiveList,
            expected_session_id: None,
        },
        "read-archives",
    )["result"]
        .clone()
}
fn show(engine: &Engine, id: &SessionId) -> Value {
    engine.success(
        D::Read {
            command: R::ArchiveShow {
                session_id: id.clone(),
                tier: GraphStatusTierV2::Verbose,
                history_before: None,
            },
            expected_session_id: None,
        },
        "read-archive",
    )["result"]
        .clone()
}
fn item_fences(engine: &Engine, id: &str) -> ItemMutationPreconditionsWireV1 {
    let graph = engine.graph();
    let attempt = graph.trace().active_attempt().unwrap().attempt_id();
    let revision = graph
        .workflow_memory()
        .attempts()
        .iter()
        .find(|a| a.attempt_id() == attempt)
        .unwrap()
        .item_slots()
        .iter()
        .find(|i| i.item_id().as_str() == id)
        .map(|i| i.revision())
        .unwrap_or(Revision::ZERO);
    ItemMutationPreconditionsWireV1 {
        expected_session_id: graph.trace().session_id().clone(),
        expected_attempt_id: attempt.clone(),
        expected_item_revision: revision,
    }
}
fn item(engine: &Engine, verb: &str, id: &str, value: &str) -> D {
    let item_id = ItemId::new(id).unwrap();
    let preconditions = item_fences(engine, id);
    D::Item(match verb {
        "check" => I::Check(ItemCheckV1 {
            item_id,
            preconditions,
        }),
        "uncheck" => I::Uncheck(ItemUncheckV1 {
            item_id,
            preconditions,
        }),
        "set" => I::Set(ItemSetV1 {
            item_id,
            value: value.into(),
            preconditions,
        }),
        "clear" => I::Clear(ItemClearV1 {
            item_id,
            preconditions,
        }),
        "add" => I::Add(ItemAddV1 {
            item_id,
            value: value.into(),
            preconditions,
        }),
        "remove" => I::Remove(ItemRemoveV1 {
            item_id,
            value: value.into(),
            ignore_missing: false,
            preconditions,
        }),
        "attach" => I::Attach(ItemAttachV1 {
            item_id,
            source: ItemAttachSourceV1::Path {
                path: value.into(),
                media_type: Some("text/plain".into()),
            },
            preconditions,
        }),
        _ => panic!("unsupported test item verb"),
    })
}
fn operation(id: &str, revision: u64, record: ItemRecordValueV1) -> ItemRecordManyOperationV1 {
    ItemRecordManyOperationV1 {
        item_id: ItemId::new(id).unwrap(),
        expected_item_revision: Revision::new(revision),
        disposition: ItemRecordManyDispositionV1::Record { record },
    }
}
fn clear(id: &str, revision: u64) -> ItemRecordManyOperationV1 {
    ItemRecordManyOperationV1 {
        item_id: ItemId::new(id).unwrap(),
        expected_item_revision: Revision::new(revision),
        disposition: ItemRecordManyDispositionV1::Clear { clear: true },
    }
}
fn batch(engine: &Engine, operations: Vec<ItemRecordManyOperationV1>) -> D {
    D::RecordMany(ItemRecordManyV1 {
        operations,
        preconditions: engine.fences(),
    })
}

#[test]
fn v2scl003_record_many_answers_every_operation_the_input_admits() {
    const OPERATIONS: usize = 70;
    let items:String=(0..OPERATIONS).map(|i|format!("      - id: item-{i:03}\n        type: confirm\n        prompt: Confirm {i}.\n        required: false\n")).collect();
    let source = format!(
        "schema: podway.procedure/v2\nid: wide-record-many\nversion: \"2\"\nname: Wide record many\npurpose: Exercise the admitted result bound.\nnode_definitions:\n  work:\n    type: action\n    title: Work\n    intent: Record items.\n    items:\n{items}graph:\n  entry: work\n  nodes:\n    - id: work\n      use: work\n      terminal: true\n"
    );
    let engine = Engine::new(&source);
    let before = engine.graph();
    let operations = (0..OPERATIONS)
        .map(|i| {
            operation(
                &format!("item-{i:03}"),
                0,
                ItemRecordValueV1::Confirm { value: true },
            )
        })
        .collect();
    let output = engine.success(batch(&engine, operations), "wide");
    assert!(
        serde_json::to_vec(&output).unwrap().len() <= podway_protocol::MAX_DIRECT_OUTPUT_BYTES_V1
    );
    assert_eq!(output["result"]["changed"], true);
    assert_eq!(
        output["result"]["items"].as_array().unwrap().len(),
        OPERATIONS
    );
    assert_eq!(
        engine.graph().trace().revision().get(),
        before.trace().revision().get() + 1
    );
}
#[test]
fn v2agt004_record_many_updates_all_item_types_atomically_and_replays_terminal_receipt() {
    let engine = Engine::new(ACTION_READBACK_PROCEDURE);
    std::fs::write(
        engine.temporary.path().join("report.txt"),
        "durable report\n",
    )
    .unwrap();
    let before = engine.graph();
    let operations = vec![
        operation(
            "summary",
            0,
            ItemRecordValueV1::Text {
                value: "atomic summary".into(),
            },
        ),
        operation(
            "report",
            0,
            ItemRecordValueV1::Artifact {
                path: Some("report.txt".into()),
                reference: None,
                digest: None,
                size_bytes: None,
                media_type: Some("text/plain".into()),
            },
        ),
        operation(
            "outcome",
            0,
            ItemRecordValueV1::Choice {
                value: "accepted".into(),
            },
        ),
        operation(
            "findings",
            0,
            ItemRecordValueV1::List {
                value: vec!["one".into(), "two".into()],
            },
        ),
        operation("count", 0, ItemRecordValueV1::Integer { value: 7 }),
        operation("confirmed", 0, ItemRecordValueV1::Confirm { value: true }),
    ];
    let command = batch(&engine, operations);
    let request = engine.request(command.clone(), "six-types");
    let original = engine.output(engine.execute(request.clone(), &Clock));
    assert_eq!(
        original["result"]["revision"],
        before.trace().revision().get() + 1
    );
    assert_eq!(original["result"]["changed"], true);
    assert_eq!(original["result"]["items"].as_array().unwrap().len(), 6);
    assert_eq!(original["result"]["items"][0]["item_id"], "confirmed");
    assert_eq!(original["result"]["items"][5]["item_id"], "summary");
    assert_eq!(
        engine.graph().trace().revision().get(),
        before.trace().revision().get() + 1
    );
    assert!(
        status(&engine)["items"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|i| i["item_id"] != "internal-note")
            .all(|i| i["satisfied"] == true)
    );
    assert_eq!(engine.output(engine.execute(request, &NoClock)), original);
    reject(
        &engine,
        batch(&engine, vec![clear("confirmed", 1), clear("summary", 0)]),
        "atomic-invalid",
        "ITEM_REVISION_CONFLICT",
    );
    reject(
        &engine,
        command,
        "stale-session-batch",
        "SESSION_REVISION_CONFLICT",
    );
}
#[test]
fn v2run003_production_actions_mutate_complete_read_back_restart_and_replay() {
    let mut engine = Engine::new(ACTION_READBACK_PROCEDURE);
    std::fs::write(
        engine.temporary.path().join("report.txt"),
        "durable report\n",
    )
    .unwrap();
    for (index, verb, id, value) in [
        (0, "check", "confirmed", ""),
        (1, "uncheck", "confirmed", ""),
        (2, "check", "confirmed", ""),
        (3, "set", "summary", "selected summary"),
        (4, "set", "outcome", "accepted"),
        (5, "set", "count", "7"),
        (6, "set", "internal-note", "must stay private"),
        (7, "add", "findings", "discarded"),
        (8, "add", "findings", "retained"),
        (9, "remove", "findings", "discarded"),
        (10, "attach", "report", "report.txt"),
        (11, "clear", "summary", ""),
    ] {
        engine.success(item(&engine, verb, id, value), &format!("item-{index}"));
    }
    reject(
        &engine,
        D::Cursor(C::Complete(SessionCompleteV1 {
            preconditions: engine.fences(),
        })),
        "missing-summary",
        "REQUIRED_ITEMS_MISSING",
    );
    engine.set("summary", "selected summary", "restore-summary");
    let capture = engine.request(
        D::Cursor(C::Complete(SessionCompleteV1 {
            preconditions: engine.fences(),
        })),
        "complete-capture",
    );
    std::fs::remove_file(engine.temporary.path().join("report.txt")).unwrap();
    reject(
        &engine,
        capture.command.clone(),
        "missing-artifact",
        "ARTIFACT_CHANGED",
    );
    reject(
        &engine,
        capture.command.clone(),
        "missing-artifact",
        "ARTIFACT_CHANGED",
    );
    std::fs::write(
        engine.temporary.path().join("report.txt"),
        "changed report\n",
    )
    .unwrap();
    reject(
        &engine,
        capture.command.clone(),
        "changed-artifact",
        "ARTIFACT_CHANGED",
    );
    reject(
        &engine,
        capture.command.clone(),
        "changed-artifact",
        "ARTIFACT_CHANGED",
    );
    std::fs::write(
        engine.temporary.path().join("report.txt"),
        "durable report\n",
    )
    .unwrap();
    let once = engine.output(engine.execute(capture.clone(), &Clock));
    assert_eq!(once["result"]["from_graph_node_id"], "capture-results");
    assert_eq!(once["result"]["to_graph_node_id"], "consume-results");
    assert_eq!(once["result"]["session_state"], "running");
    let mut replay = capture;
    replay.request_id = RequestIdV1::new("00000000-0000-4000-8000-000000000098").unwrap();
    assert_eq!(engine.output(engine.execute(replay, &NoClock)), once);
    let graph = engine.graph();
    let source_digest = graph.workflow_memory().attempts()[0]
        .recorded_items_digest()
        .unwrap();
    let next = engine.read(R::Next);
    let readback = next["readback"].as_array().unwrap();
    assert_eq!(readback.len(), 2);
    assert_eq!(
        readback[0]["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["item_id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["summary", "findings", "report"]
    );
    assert_eq!(readback[0]["items_digest"], source_digest.as_str());
    assert_eq!(readback[1]["source_graph_node_id"], "finish");
    assert_eq!(readback[1]["state"], "unresolved");
    assert!(readback[1]["items"].as_array().unwrap().is_empty());
    let observed = engine.read(R::Observe);
    assert_eq!(
        observed["status"]["session"]["id"],
        json!(graph.trace().session_id())
    );
    let guidance = observed["guidance"]["readback"].as_array().unwrap();
    assert_eq!(guidance.len(), readback.len());
    for (actual, expected) in guidance.iter().zip(readback) {
        assert_eq!(
            actual["source_graph_node_id"],
            expected["source_graph_node_id"]
        );
        let a = actual["items"].as_array().unwrap();
        let b = expected["items"].as_array().unwrap();
        assert_eq!(a.len(), b.len());
        for (a, b) in a.iter().zip(b) {
            for field in ["item_id", "type", "revision", "value_digest", "total_size"] {
                assert_eq!(a[field], b[field]);
            }
            for absent in ["preview", "preview_truncated", "next_page_token"] {
                assert!(a.get(absent).is_none());
            }
        }
    }
    assert_eq!(observed["active_items"].as_array().unwrap().len(), 1);
    assert!(
        observed["mutation_templates"]
            .as_array()
            .unwrap()
            .iter()
            .all(|t| t["idempotency_key_required"] == true)
    );
    std::fs::remove_file(engine.temporary.path().join("source.yaml")).unwrap();
    std::fs::remove_file(engine.temporary.path().join("report.txt")).unwrap();
    engine.cold();
    assert_eq!(engine.read(R::Next)["readback"], next["readback"]);
    engine.success(item(&engine, "check", "consumed", ""), "consumed");
    assert_eq!(
        engine.complete("complete-consume")["result"]["to_graph_node_id"],
        "finish"
    );
    let final_result = engine.complete("complete-finish");
    assert_eq!(final_result["result"]["from_graph_node_id"], "finish");
    assert_eq!(final_result["result"]["session_state"], "completed");
}
#[test]
fn v2lif004_prepared_begin_disposition_and_eligible_reset_form_one_runtime_flow() {
    let engine = prepared(ACTION_READBACK_PROCEDURE);
    let started = engine.success(
        D::OutcomeLookup {
            key: IdempotencyKeyV1::new("start").unwrap(),
        },
        "lookup",
    )["result"]["outcome"]["response"]["result"]
        .clone();
    assert_eq!(started["schema"], "podway.session-start-result/v4");
    assert_eq!(started["session_state"], "prepared");
    assert_eq!(started["revision"], 0);
    assert!(started["active_attempt"].is_null());
    assert_eq!(
        engine.graph().trace().lifecycle(),
        SessionLifecycle::Prepared
    );
    assert_eq!(engine.graph().trace().revision(), Revision::ZERO);
    assert!(status(&engine)["current"].is_null());
    let fence = identity(&engine);
    for (key, preconditions, code) in [
        (
            "begin-stale",
            SessionIdentityPreconditionsWireV1 {
                expected_session_revision: Revision::new(1),
                ..fence.clone()
            },
            "SESSION_REVISION_CONFLICT",
        ),
        (
            "begin-identity",
            SessionIdentityPreconditionsWireV1 {
                expected_session_id: SessionId::new("00000000-0000-4000-8000-000000149996")
                    .unwrap(),
                ..fence.clone()
            },
            "SESSION_ID_MISMATCH",
        ),
    ] {
        reject(
            &engine,
            D::Procedure(ProcedureV2MutationCommandV1::SessionBegin(SessionBeginV1 {
                initial_goal: None,
                preconditions,
            })),
            key,
            code,
        );
    }
    reject(
        &engine,
        D::Cursor(C::Cancel(SessionCancelV1 {
            reason: "Prepared has no work".into(),
            preconditions: SessionMutationPreconditionsWireV1 {
                expected_session_id: fence.expected_session_id.clone(),
                expected_session_revision: Revision::ZERO,
                expected_attempt_id: AttemptId::new("00000000-0000-4000-8000-000000014999")
                    .unwrap(),
            },
        })),
        "cancel-prepared",
        "SESSION_NOT_RUNNING",
    );
    let begun = begin(&engine, "begin");
    assert_eq!(begun["result"]["schema"], "podway.session-begin-result/v2");
    assert_eq!(begun["result"]["session_state"], "running");
    assert_eq!(begun["result"]["revision"], 1);
    assert_eq!(begun["result"]["goal_defined"], false);
    reject(
        &engine,
        D::Procedure(ProcedureV2MutationCommandV1::SessionBegin(SessionBeginV1 {
            initial_goal: None,
            preconditions: identity(&engine),
        })),
        "begin-again",
        "SESSION_NOT_RUNNING",
    );
    reject(
        &engine,
        disposition(&engine),
        "running-disposition",
        "SESSION_NOT_TERMINAL",
    );
    reject(
        &engine,
        reset(&engine, SessionDeletionModeV1::Eligible, false),
        "running-reset",
        "SESSION_RESET_NOT_ELIGIBLE",
    );
    assert_eq!(
        cancel(&engine, "cancel")["result"]["session_state"],
        "cancelled"
    );
    let terminal = identity(&engine);
    let stale = SessionIdentityPreconditionsWireV1 {
        expected_session_revision: Revision::new(terminal.expected_session_revision.get() - 1),
        ..terminal.clone()
    };
    let make = |preconditions| {
        D::Procedure(ProcedureV2MutationCommandV1::SessionTerminalDisposition(
            SessionTerminalDispositionV1 {
                disposition: TerminalDispositionInputV1::HandedOff {
                    summary: "Delivered the prepared lifecycle result.".into(),
                    reference: "commit:v2lif005".into(),
                },
                actor: Some("integration-test".into()),
                preconditions,
            },
        ))
    };
    reject(
        &engine,
        make(stale.clone()),
        "stale-disposition",
        "SESSION_REVISION_CONFLICT",
    );
    let result = engine.success(make(terminal), "disposition");
    assert_eq!(
        result["result"]["schema"],
        "podway.terminal-disposition-result/v2"
    );
    for (field, value) in [
        ("kind", "handed_off"),
        ("summary", "Delivered the prepared lifecycle result."),
        ("reference", "commit:v2lif005"),
        ("actor", "integration-test"),
    ] {
        assert_eq!(result["result"]["disposition"][field], value);
    }
    reject(
        &engine,
        disposition(&engine),
        "duplicate-disposition",
        "REQUEST_INVALID",
    );
    reject(
        &engine,
        make(stale),
        "stale-duplicate",
        "SESSION_REVISION_CONFLICT",
    );
    let reset = engine.success(
        reset(&engine, SessionDeletionModeV1::Eligible, false),
        "reset",
    );
    assert_eq!(reset["result"]["mode"], "eligible");
    assert_eq!(reset["result"]["reset"], true);
    assert!(current(&engine).is_none());
}
#[test]
fn v2lif005_prepared_session_dry_run_and_eligible_reset_need_no_force_summary() {
    let engine = prepared(ACTION_READBACK_PROCEDURE);
    let goal = InitialGoalWireV2 {
        actor: None,
        goal: "Prepared goal".into(),
        criteria: vec![GoalCriterionWireV2 {
            criterion_id: podway_core::CriterionId::new("verified").unwrap(),
            statement: "Verify the isolated task".into(),
        }],
    };
    reject(
        &engine,
        D::Procedure(ProcedureV2MutationCommandV1::SessionBegin(SessionBeginV1 {
            initial_goal: Some(goal),
            preconditions: identity(&engine),
        })),
        "goal-disabled",
        "GOAL_TRACKING_NOT_ENABLED",
    );
    let before = engine.graph();
    let sequence = engine.sequence();
    let preview = engine.success(
        reset(&engine, SessionDeletionModeV1::Eligible, true),
        "preview",
    );
    assert_eq!(preview["result"]["lifecycle"], "prepared");
    assert_eq!(preview["result"]["eligible"], true);
    assert_eq!(preview["result"]["required_action"], "none");
    assert_eq!(preview["result"]["reset"], false);
    assert_eq!(engine.graph(), before);
    assert_eq!(engine.sequence(), sequence);
    let id = before.trace().session_id().clone();
    assert_eq!(
        engine.success(
            reset(&engine, SessionDeletionModeV1::Eligible, false),
            "reset"
        )["result"]["reset"],
        true
    );
    reject(
        &engine,
        D::Read {
            command: R::Status {
                tier: GraphStatusTierV2::Standard,
                history_before: None,
            },
            expected_session_id: Some(id),
        },
        "old-status",
        "SESSION_ID_MISMATCH",
    );
}
#[test]
fn v2lif005_eligible_replacement_dry_run_uses_current_reset_eligibility() {
    let engine = empty(ACTION_READBACK_PROCEDURE);
    let absent = SessionIdentityPreconditionsWireV1 {
        expected_session_id: SessionId::new("00000000-0000-4000-8000-000000143002").unwrap(),
        expected_session_revision: Revision::ZERO,
    };
    reject(
        &engine,
        D::Start(T::Replace(SessionStartReplaceV1 {
            start: start(&engine, true),
            mode: SessionDeletionModeV1::Eligible,
            preconditions: absent,
        })),
        "absent-preview",
        "SESSION_ID_MISMATCH",
    );
    engine.success(D::Start(T::Start(start(&engine, false))), "start");
    let before = engine.graph();
    let sequence = engine.sequence();
    assert_eq!(
        engine.success(
            replace(&engine, SessionDeletionModeV1::Eligible, true),
            "prepared-preview"
        )["result"]["dry_run"],
        true
    );
    assert_eq!(engine.graph(), before);
    assert_eq!(engine.sequence(), sequence);
    begin(&engine, "begin");
    reject(
        &engine,
        replace(&engine, SessionDeletionModeV1::Eligible, true),
        "running-preview",
        "SESSION_RESET_NOT_ELIGIBLE",
    );
    cancel(&engine, "cancel");
    reject(
        &engine,
        replace(&engine, SessionDeletionModeV1::Eligible, true),
        "undisposed-preview",
        "SESSION_RESET_NOT_ELIGIBLE",
    );
    let archived = identity(&engine);
    engine.success(disposition(&engine), "disposition");
    let before = engine.graph();
    let sequence = engine.sequence();
    assert_eq!(
        engine.success(
            replace(&engine, SessionDeletionModeV1::Eligible, true),
            "disposed-preview"
        )["result"]["dry_run"],
        true
    );
    assert_eq!(engine.graph(), before);
    assert_eq!(engine.sequence(), sequence);
    assert_eq!(
        engine.success(
            replace(&engine, SessionDeletionModeV1::Eligible, false),
            "replace"
        )["result"]["session_state"],
        "prepared"
    );
    let list = archives(&engine);
    assert_eq!(list["count"], 1);
    assert_eq!(
        list["sessions"][0]["session_id"],
        json!(archived.expected_session_id)
    );
    assert_eq!(
        list["sessions"][0]["session_revision"],
        json!(archived.expected_session_revision)
    );
    let shown = show(&engine, &archived.expected_session_id);
    assert_eq!(shown["schema"], "podway.session-archive-show-result/v3");
    assert_eq!(shown["activity"], "inactive");
    assert_eq!(
        shown["state"]["session"]["id"],
        json!(archived.expected_session_id)
    );
    assert_eq!(shown["state"]["session"]["lifecycle"], "cancelled");
    assert_eq!(
        engine.success(
            D::Session(S::Purge(SessionArchivePurgeV1 {
                session_id: archived.expected_session_id.clone(),
                expected_session_revision: archived.expected_session_revision,
                confirmed: true
            })),
            "purge"
        )["result"]["purged"],
        true
    );
    assert_eq!(archives(&engine)["count"], 0);
    let error = reject(
        &engine,
        D::Read {
            command: R::ArchiveShow {
                session_id: archived.expected_session_id.clone(),
                tier: GraphStatusTierV2::Standard,
                history_before: None,
            },
            expected_session_id: None,
        },
        "purged-read",
        "SESSION_ARCHIVE_NOT_FOUND",
    );
    assert_eq!(
        error["details"]["session_id"],
        json!(archived.expected_session_id)
    );
}
#[test]
fn session_archive_is_durably_admitted_and_reaches_the_v8_commit_path() {
    let mut engine = Engine::new(STATE_PROCEDURE);
    cancel(&engine, "cancel");
    let id = engine.graph().trace().session_id().clone();
    engine.success(disposition(&engine), "disposition");
    let request = engine.request(
        D::Session(S::Archive(SessionArchiveV1 {
            preconditions: identity(&engine),
        })),
        "archive",
    );
    let once = engine.output(engine.execute(request.clone(), &Clock));
    assert_eq!(once["result"]["activity"], "inactive");
    assert_eq!(once["result"]["session_id"], json!(id));
    assert_eq!(once["result"]["lifecycle"], "cancelled");
    assert!(current(&engine).is_none());
    engine.cold();
    assert_eq!(engine.output(engine.execute(request, &NoClock)), once);
    assert_eq!(archives(&engine)["count"], 1);
    assert_eq!(
        show(&engine, &id)["state"]["session"]["lifecycle"],
        "cancelled"
    );
}
#[test]
fn ineligible_session_archive_terminalizes_without_poisoning_the_workspace() {
    let mut engine = prepared(STATE_PROCEDURE);
    let before = status(&engine);
    reject(
        &engine,
        D::Session(S::Archive(SessionArchiveV1 {
            preconditions: identity(&engine),
        })),
        "ineligible",
        "SESSION_ARCHIVE_NOT_ELIGIBLE",
    );
    engine.cold();
    assert_eq!(status(&engine), before);
    assert_eq!(
        begin(&engine, "still-usable")["result"]["session_state"],
        "running"
    );
}
#[test]
fn v2lif004_eligible_and_force_replacement_then_force_reset_are_atomic() {
    let engine = prepared(ACTION_READBACK_PROCEDURE);
    let first = engine.graph().trace().session_id().clone();
    let replaced = engine.success(
        replace(&engine, SessionDeletionModeV1::Eligible, false),
        "replace-eligible",
    );
    assert_eq!(replaced["result"]["session_state"], "prepared");
    assert_ne!(replaced["result"]["session_id"], json!(first));
    begin(&engine, "begin-second");
    let second = engine.graph().trace().session_id().clone();
    let replaced = engine.success(
        replace(
            &engine,
            SessionDeletionModeV1::Force {
                progress_summary: "The isolated test intentionally discards its work.".into(),
            },
            false,
        ),
        "replace-force",
    );
    assert_eq!(replaced["result"]["session_state"], "prepared");
    assert_ne!(replaced["result"]["session_id"], json!(second));
    begin(&engine, "begin-third");
    let reset = engine.success(
        reset(
            &engine,
            SessionDeletionModeV1::Force {
                progress_summary: "The isolated test intentionally discards its work.".into(),
            },
            false,
        ),
        "reset-force",
    );
    assert_eq!(reset["result"]["mode"], "force");
    assert_eq!(reset["result"]["lifecycle"], "running");
    assert_eq!(reset["result"]["reset"], true);
    assert!(current(&engine).is_none());
}
#[test]
fn start_supersede_atomically_archives_running_session_and_prepares_successor() {
    let engine = Engine::new(ACTION_READBACK_PROCEDURE);
    let id = engine.graph().trace().session_id().clone();
    let mode = SessionDeletionModeV1::Supersede {
        reason: "The successor task now has priority.".into(),
        actor: Some("developer".into()),
    };
    let before = engine.graph();
    let sequence = engine.sequence();
    let preview = engine.success(replace(&engine, mode.clone(), true), "preview");
    assert_eq!(preview["result"]["dry_run"], true);
    assert_eq!(
        preview["result"]["existing_session"]["lifecycle"],
        "running"
    );
    assert_eq!(
        preview["result"]["existing_session"]["action"],
        "supersede_archive"
    );
    assert_eq!(engine.graph(), before);
    assert_eq!(engine.sequence(), sequence);
    let successor = engine.success(replace(&engine, mode, false), "supersede");
    assert_eq!(successor["result"]["session_state"], "prepared");
    assert_ne!(successor["result"]["session_id"], json!(id));
    let list = archives(&engine);
    assert_eq!(list["count"], 1);
    assert_eq!(list["sessions"][0]["session_id"], json!(id));
    assert_eq!(list["sessions"][0]["lifecycle"], "cancelled");
    assert_eq!(list["sessions"][0]["session_revision"], 2);
    let shown = show(&engine, &id);
    for (field, value) in [
        ("kind", "superseded"),
        ("reason", "The successor task now has priority."),
        ("actor", "developer"),
    ] {
        assert_eq!(shown["terminal_disposition"][field], value);
    }
    assert_eq!(
        shown["terminal_disposition"]["successor_session_id"],
        successor["result"]["session_id"]
    );
}
#[test]
fn unknown_preset_start_and_replace_fail_without_admission_or_state_change() {
    let engine = empty(ACTION_READBACK_PROCEDURE);
    let unknown = |dry_run| SessionStartV1 {
        source: SessionStartSourceV1::Preset {
            preset: "aquarium-goal-v2".into(),
        },
        expected_procedure_digest: None,
        task_title: "Unknown preset".into(),
        dry_run,
    };
    for dry_run in [true, false] {
        let error = reject(
            &engine,
            D::Start(T::Start(unknown(dry_run))),
            "unknown-start",
            "PRESET_NOT_FOUND",
        );
        assert_eq!(error["command"], "session.start");
        assert_eq!(error["exit_code"], 1);
    }
    engine.success(D::Start(T::Start(start(&engine, false))), "start");
    begin(&engine, "begin");
    for dry_run in [true, false] {
        let error = reject(
            &engine,
            D::Start(T::Replace(SessionStartReplaceV1 {
                start: unknown(dry_run),
                mode: SessionDeletionModeV1::Supersede {
                    reason: "The successor has priority.".into(),
                    actor: None,
                },
                preconditions: identity(&engine),
            })),
            "unknown-replace",
            "PRESET_NOT_FOUND",
        );
        assert_eq!(error["command"], "session.start_replace");
        assert_eq!(error["exit_code"], 1);
    }
    assert_eq!(engine.graph().trace().revision(), Revision::new(1));
    assert_eq!(
        engine.graph().trace().lifecycle(),
        SessionLifecycle::Running
    );
    assert_eq!(archives(&engine)["count"], 0);
}
#[test]
fn state_aware_start_delete_shape_rejects_before_durable_admission() {
    let engine = prepared(ACTION_READBACK_PROCEDURE);
    reject(
        &engine,
        replace(
            &engine,
            SessionDeletionModeV1::Delete {
                confirmed: true,
                progress_summary: Some("Prepared state has no work to summarize.".into()),
            },
            false,
        ),
        "delete-prepared",
        "REQUEST_INVALID",
    );
    engine.success(
        replace(
            &engine,
            SessionDeletionModeV1::Delete {
                confirmed: true,
                progress_summary: None,
            },
            false,
        ),
        "delete-prepared",
    );
    begin(&engine, "begin-replacement");
    reject(
        &engine,
        replace(
            &engine,
            SessionDeletionModeV1::Delete {
                confirmed: true,
                progress_summary: None,
            },
            false,
        ),
        "delete-running",
        "REQUEST_INVALID",
    );
    assert_eq!(
        engine.success(
            replace(
                &engine,
                SessionDeletionModeV1::Delete {
                    confirmed: true,
                    progress_summary: Some(
                        "The running work was summarized before deletion.".into()
                    )
                },
                false
            ),
            "delete-running"
        )["result"]["session_state"],
        "prepared"
    );
}

const CHECK_PROCEDURE: &str = r#"schema: podway.procedure/v2
id: check-result-runtime
version: "2"
name: Check result runtime
purpose: Prove durable external check result recording.
node_definitions:
  verify:
    type: action
    title: Verify
    intent: Record structurally bound external results.
    items:
      - id: passed
        type: check_result
        prompt: Record a passing check.
        required: true
        operation_id: pass-check
        operation_digest: sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
        accepted_outcomes: [pass]
      - id: failed
        type: check_result
        prompt: Record a failing check.
        required: true
        operation_id: fail-check
        operation_digest: sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
        accepted_outcomes: [fail]
      - id: inconclusive
        type: check_result
        prompt: Record an inconclusive check.
        required: true
        operation_id: inconclusive-check
        operation_digest: sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc
        accepted_outcomes: [inconclusive]
      - id: identity
        type: check_result
        prompt: Record a result bound to this operation.
        required: true
        operation_id: identity-check
        operation_digest: sha256:dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd
        accepted_outcomes: [pass]
graph:
  entry: verify
  nodes:
    - id: verify
      use: verify
      terminal: true
"#;
fn check_result(
    operation_id: &str,
    nibble: char,
    outcome: podway_core::CheckResultOutcomeV2,
) -> ItemRecordValueV1 {
    use podway_core::{
        CheckResultExecutorV2, CheckResultInputBasisV2, CheckResultValueV2, OperationId,
        Sha256Digest,
    };
    let hash = |c: char| Sha256Digest::new(format!("sha256:{}", c.to_string().repeat(64))).unwrap();
    let label = match outcome {
        podway_core::CheckResultOutcomeV2::Pass => "pass",
        podway_core::CheckResultOutcomeV2::Fail => "fail",
        podway_core::CheckResultOutcomeV2::Inconclusive => "inconclusive",
    };
    ItemRecordValueV1::CheckResult {
        value: CheckResultValueV2::new(
            OperationId::new(operation_id).unwrap(),
            hash(nibble),
            CheckResultInputBasisV2::new("HEAD and dirty-tree snapshot", hash('e')).unwrap(),
            CheckResultExecutorV2::new("gaori", "1.0.0").unwrap(),
            outcome,
            format!("The external operation reported {label}."),
            hash('f'),
        )
        .unwrap(),
    }
}
#[test]
fn v2ast004_check_results_record_atomically_replay_restart_and_expire_on_retry() {
    use podway_core::CheckResultOutcomeV2::{Fail, Inconclusive, Pass};
    let mut engine = Engine::new(CHECK_PROCEDURE);
    let request = engine.request(
        batch(
            &engine,
            vec![
                operation("passed", 0, check_result("pass-check", 'a', Pass)),
                operation("failed", 0, check_result("fail-check", 'b', Fail)),
                operation(
                    "inconclusive",
                    0,
                    check_result("inconclusive-check", 'c', Inconclusive),
                ),
                operation("identity", 0, check_result("wrong-operation", 'd', Pass)),
            ],
        ),
        "check-results",
    );
    let once = engine.output(engine.execute(request.clone(), &Clock));
    assert_eq!(once["result"]["changed"], true);
    assert_eq!(once["result"]["items"].as_array().unwrap().len(), 4);
    assert_eq!(engine.output(engine.execute(request, &NoClock)), once);
    let first = status(&engine);
    for id in ["passed", "failed", "inconclusive"] {
        assert_eq!(
            first["items"]
                .as_array()
                .unwrap()
                .iter()
                .find(|i| i["item_id"] == id)
                .unwrap()["satisfied"],
            true
        );
    }
    assert_eq!(
        first["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["item_id"] == "identity")
            .unwrap()["satisfied"],
        false
    );
    reject(
        &engine,
        batch(
            &engine,
            vec![
                operation("identity", 1, check_result("identity-check", 'd', Pass)),
                clear("passed", 0),
            ],
        ),
        "atomic-check-conflict",
        "ITEM_REVISION_CONFLICT",
    );
    assert_eq!(status(&engine)["items"], first["items"]);
    engine.success(
        batch(
            &engine,
            vec![operation(
                "identity",
                1,
                check_result("identity-check", 'd', Pass),
            )],
        ),
        "repair-identity",
    );
    let durable = status(&engine);
    assert!(
        durable["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|i| i["satisfied"] == true)
    );
    engine.cold();
    assert_eq!(status(&engine)["items"], durable["items"]);
    let graph = engine.graph();
    let value = graph
        .workflow_memory()
        .attempts()
        .last()
        .unwrap()
        .item_slots()
        .iter()
        .find(|i| i.item_id().as_str() == "identity")
        .unwrap()
        .value()
        .unwrap()
        .as_check_result()
        .unwrap();
    assert_eq!(value.operation_id().as_str(), "identity-check");
    assert_eq!(value.summary(), "The external operation reported pass.");
    let old = engine.fences().expected_attempt_id;
    engine.success(
        engine.retry("prove stale attempt-local results are not current".into()),
        "retry",
    );
    assert_ne!(engine.fences().expected_attempt_id, old);
    assert!(
        status(&engine)["items"]
            .as_array()
            .unwrap()
            .iter()
            .all(|i| i["revision"] == 0 && i["satisfied"] == false && i.get("value").is_none())
    );
}
#[test]
fn v2lif008_prepared_item_mutations_fail_before_attempt_and_item_fences() {
    let engine = prepared(ACTION_READBACK_PROCEDURE);
    let old = AttemptId::new("00000000-0000-4000-8000-000000000099").unwrap();
    let p = ItemMutationPreconditionsWireV1 {
        expected_session_id: identity(&engine).expected_session_id,
        expected_attempt_id: old.clone(),
        expected_item_revision: Revision::new(99),
    };
    let id = |id| ItemId::new(id).unwrap();
    let commands = vec![
        I::Check(ItemCheckV1 {
            item_id: id("confirmed"),
            preconditions: p.clone(),
        }),
        I::Uncheck(ItemUncheckV1 {
            item_id: id("confirmed"),
            preconditions: p.clone(),
        }),
        I::Set(ItemSetV1 {
            item_id: id("summary"),
            value: "prepared".into(),
            preconditions: p.clone(),
        }),
        I::Add(ItemAddV1 {
            item_id: id("findings"),
            value: "prepared".into(),
            preconditions: p.clone(),
        }),
        I::Remove(ItemRemoveV1 {
            item_id: id("findings"),
            value: "prepared".into(),
            ignore_missing: false,
            preconditions: p.clone(),
        }),
        I::Attach(ItemAttachV1 {
            item_id: id("report"),
            source: ItemAttachSourceV1::Path {
                path: "report.txt".into(),
                media_type: Some("text/plain".into()),
            },
            preconditions: p.clone(),
        }),
        I::Clear(ItemClearV1 {
            item_id: id("summary"),
            preconditions: p.clone(),
        }),
    ];
    for (n, command) in commands.into_iter().enumerate() {
        reject(
            &engine,
            D::Item(command),
            &format!("prepared-{n}"),
            "SESSION_NOT_RUNNING",
        );
    }
    let command = |revision| {
        D::RecordMany(ItemRecordManyV1 {
            operations: vec![operation(
                "summary",
                99,
                ItemRecordValueV1::Text {
                    value: "prepared".into(),
                },
            )],
            preconditions: SessionMutationPreconditionsWireV1 {
                expected_session_id: p.expected_session_id.clone(),
                expected_session_revision: revision,
                expected_attempt_id: old.clone(),
            },
        })
    };
    reject(
        &engine,
        command(Revision::ZERO),
        "prepared-batch",
        "SESSION_NOT_RUNNING",
    );
    begin(&engine, "begin");
    reject(
        &engine,
        D::Item(I::Check(ItemCheckV1 {
            item_id: id("confirmed"),
            preconditions: p.clone(),
        })),
        "running-stale-attempt",
        "ATTEMPT_NOT_CURRENT",
    );
    reject(
        &engine,
        command(Revision::new(1)),
        "running-stale-batch",
        "ATTEMPT_NOT_CURRENT",
    );
}
#[test]
fn v2rel003_detached_start_lookup_and_terminal_replay_use_common_automation_pipeline() {
    let mut engine = empty(ACTION_READBACK_PROCEDURE);
    let request = engine.request(D::Start(T::Start(start(&engine, false))), "start");
    let once = engine.output(engine.execute(request.clone(), &Clock));
    assert_eq!(once["result"]["session_state"], "prepared");
    std::fs::remove_file(engine.temporary.path().join("source.yaml")).unwrap();
    engine.cold();
    let sequence = engine.sequence();
    let lookup = engine.success(
        D::OutcomeLookup {
            key: IdempotencyKeyV1::new("start").unwrap(),
        },
        "lookup",
    );
    assert_eq!(lookup["result"]["found"], true);
    assert_eq!(lookup["result"]["outcome"]["response"], once);
    assert_eq!(engine.output(engine.execute(request, &NoClock)), once);
    assert_eq!(engine.sequence(), sequence);
}
fn seed(engine: &Engine, graph: &GraphSessionStateV2, key: &str) {
    use podway_store::{
        CanonicalExecutionJsonV1,
        direct_store::{DirectMutationRequestV1, DirectOutcomeScopeV1},
    };
    let request = DirectMutationRequestV1::new(
        IdempotencyKeyV1::new(key).unwrap(),
        digest(),
        "workspace.repair",
        "00000000-0000-4000-8000-000000000002",
        DirectOutcomeScopeV1::Workspace,
    )
    .unwrap();
    engine.store.mutate(&request,deadline(),||UnixMillis::new(1700000000005),|txn| {
        txn.now();txn.create_graph(graph)?;
        Ok(CanonicalExecutionJsonV1::new(podway_core::canonicalize_json_v1(&json!({"schema":"podway.output/v4","request_id":"00000000-0000-4000-8000-000000000002","command":"workspace.repair","generated_at":"2023-11-14T22:13:20.005Z","result":{},"warnings":[]})).unwrap()).unwrap())
    }).unwrap();
}
#[test]
fn session_archive_list_windows_two_hundred_fifty_seven_inactive_sessions() {
    let engine = empty(STATE_PROCEDURE);
    let mut ids = Vec::new();
    for offset in 0..257 {
        let graph = podway_store::test_support::running_archive_fixture_session_v2(
            20000 + offset,
            30000 + offset,
            10,
        );
        seed(&engine, &graph, &format!("seed-{offset}"));
        engine.complete(&format!("complete-{offset}"));
        engine.success(disposition(&engine), &format!("dispose-{offset}"));
        ids.push(engine.graph().trace().session_id().clone());
        engine.success(
            D::Session(S::Archive(SessionArchiveV1 {
                preconditions: identity(&engine),
            })),
            &format!("archive-{offset}"),
        );
    }
    let list = archives(&engine);
    assert_eq!(list["count"], 257);
    assert_eq!(list["truncated"], true);
    assert_eq!(list["sessions"].as_array().unwrap().len(), 256);
    assert_eq!(list["sessions"][0]["session_id"], json!(ids[256]));
    assert_eq!(list["sessions"][0]["archive_slot"], 257);
    assert_eq!(list["sessions"][255]["session_id"], json!(ids[1]));
    assert_eq!(list["sessions"][255]["archive_slot"], 2);
    assert_eq!(list["oldest"]["session_id"], json!(ids[0]));
    assert_eq!(list["oldest"]["archive_slot"], 1);
}
#[test]
fn doctor_reports_unreadable_state_without_activating_a_scheduler() {
    let mut engine = Engine::new(ACTION_READBACK_PROCEDURE);
    let database = engine.store.layout().database();
    podway_store::test_support::corrupt_current_session_cursor(&database).unwrap();
    engine.cold();
    let sequence = engine.sequence();
    let DirectResponseV1::Failure(error) = engine.execute(
        engine.request(
            D::Read {
                command: R::Status {
                    tier: GraphStatusTierV2::Standard,
                    history_before: None,
                },
                expected_session_id: None,
            },
            "corrupt-status",
        ),
        &Clock,
    ) else {
        panic!("corrupt status must fail closed")
    };
    assert_eq!(error.into_value()["code"], "WORKSPACE_STATE_UNREADABLE");
    for deep in [true, false] {
        let doctor = engine.success(D::WorkspaceDoctor { deep }, "doctor")["result"].clone();
        assert_eq!(doctor["healthy"], false);
        assert_eq!(doctor["deep"], deep);
        assert!(
            doctor["checks"]
                .as_array()
                .unwrap()
                .iter()
                .any(|c| c["id"] == "storage_integrity"
                    && c["status"] == "fail"
                    && c["reason"] == "STORAGE_INTEGRITY_FAILED")
        );
        let encoded = serde_json::to_string(&doctor).unwrap();
        assert!(!encoded.contains(engine.temporary.path().to_str().unwrap()));
        assert!(doctor.get("queue").is_none());
    }
    assert_eq!(engine.sequence(), sequence);
}
#[test]
fn deep_doctor_reports_a_session_projection_failure() {
    use podway_core::{BlockerId, ProcedureSnapshotId};
    let engine = empty(ACTION_READBACK_PROCEDURE);
    let snapshot = podway_engine::execution::workspace_procedure_snapshot_from_bytes_v2(
        "source.yaml",
        ACTION_READBACK_PROCEDURE.as_bytes(),
        ProcedureSnapshotId::new("00000000-0000-4000-8000-000000145101").unwrap(),
        UnixMillis::new(1),
    )
    .unwrap();
    let running = GraphSessionStateV2::prepared(
        Revision::new(1),
        "Diagnose a projection failure",
        snapshot,
        SessionId::new("00000000-0000-4000-8000-000000145102").unwrap(),
        UnixMillis::new(1),
    )
    .unwrap()
    .begin_v2(
        Revision::ZERO,
        AttemptId::new("00000000-0000-4000-8000-000000145103").unwrap(),
        None,
        UnixMillis::new(2),
    )
    .unwrap()
    .into_state();
    let graph = running
        .block_active_attempt_v2(
            running.trace().revision(),
            running.trace().active_attempt().unwrap().attempt_id(),
            BlockerId::new("00000000-0000-4000-8000-000000145104").unwrap(),
            "The timestamp is intentionally outside the public projection range.",
            UnixMillis::new(i64::MAX as u64),
        )
        .unwrap()
        .into_state();
    seed(&engine, &graph, "seed-projection");
    let sequence = engine.sequence();
    let doctor = engine.success(D::WorkspaceDoctor { deep: true }, "doctor")["result"].clone();
    assert_eq!(doctor["healthy"], false);
    assert_eq!(doctor["deep"], true);
    let checks = doctor["checks"].as_array().unwrap();
    assert!(
        checks
            .iter()
            .any(|c| c["id"] == "storage_integrity" && c["status"] == "pass")
    );
    assert!(checks.iter().any(|c| c["id"] == "public_status_projection"
        && c["status"] == "fail"
        && c["reason"] == "PUBLIC_PROJECTION_INVALID"));
    assert_eq!(engine.sequence(), sequence);
    assert_eq!(engine.graph(), graph);
}
#[test]
fn v2wal001_previews_use_the_active_store_when_wal_paths_are_unlinked() {
    // Direct connections do not retain a daemon-owned WAL snapshot. The retained requirement is
    // that previews and diagnostics share the current authoritative Store and remain write-free.
    let mut engine = prepared(ACTION_READBACK_PROCEDURE);
    engine.cold();
    let before = engine.graph();
    let sequence = engine.sequence();
    let preview = engine.success(
        reset(&engine, SessionDeletionModeV1::Eligible, true),
        "reset-preview",
    );
    assert_eq!(preview["result"]["session_revision"], 0);
    assert_eq!(preview["result"]["lifecycle"], "prepared");
    assert_eq!(preview["result"]["eligible"], true);
    assert_eq!(
        engine.success(
            replace(&engine, SessionDeletionModeV1::Eligible, true),
            "replace-preview"
        )["result"]["dry_run"],
        true
    );
    assert_eq!(
        engine.success(D::WorkspaceDoctor { deep: true }, "doctor")["result"]["healthy"],
        true
    );
    assert_eq!(engine.graph(), before);
    assert_eq!(engine.sequence(), sequence);
}
#[test]
fn v2sdg006_delegated_archived_session_show_uses_v2_result() {
    let source = json!({"schema":"podway.procedure/v2","id":"delegated-archive-show","version":"1","name":"Delegated archive show","purpose":"Exercise the retained delegated result.",
        "node_definitions":{"work":{"type":"action","title":"Work","intent":"Review work."}},
        "graph":{"entry":"work","nodes":[{"id":"work","use":"work","terminal":true,"delegation":{"binding_ref":"local-specialist","role_ref":"reviewer","role_snapshot_digest":digest(),"max_rounds":3,"after_accept":"retain"}}]}});
    let mut engine = empty(&serde_json::to_string(&source).unwrap());
    // Retain the original supported preseeded snapshot fixture, then exercise all reads
    // and acceptance at the actual direct API.
    let canonical = podway_core::canonicalize_json_v1(&source).unwrap();
    use sha2::{Digest as _, Sha256};
    let snapshot = podway_store::admit_procedure_snapshot_v2(
        podway_core::ProcedureSnapshotId::new("00000000-0000-4000-8000-000000015101").unwrap(),
        podway_core::CanonicalProcedureJsonV1::new(canonical.clone()).unwrap(),
        podway_core::Sha256Digest::new(format!(
            "sha256:{:x}",
            Sha256::digest(canonical.as_bytes())
        ))
        .unwrap(),
        podway_core::ProcedureSourceLabelV1::file("delegated-archive-show.yaml").unwrap(),
        UnixMillis::new(2),
    )
    .unwrap();
    let running = GraphSessionStateV2::prepared(
        Revision::new(1),
        "Delegated archive show",
        snapshot,
        SessionId::new("00000000-0000-4000-8000-000000015102").unwrap(),
        UnixMillis::new(3),
    )
    .unwrap()
    .begin_v2(
        Revision::ZERO,
        AttemptId::new("00000000-0000-4000-8000-000000015103").unwrap(),
        None,
        UnixMillis::new(4),
    )
    .unwrap()
    .into_state();
    seed(&engine, &running, "seed-delegation");
    let live = |engine: &Engine, state: &str, total: u64| {
        let status = status(engine);
        assert_eq!(status["schema"], "podway.status-result/v5");
        assert_eq!(status["delegation"]["state"], state);
        assert_eq!(status["receipt_summaries_total"], total);
        let next = engine.read(R::Next);
        assert_eq!(next["schema"], "podway.next-result/v5");
        assert_eq!(next["delegation"], status["delegation"]);
        let observe = engine.read(R::Observe);
        assert_eq!(observe["status"]["delegation"], status["delegation"]);
        assert_eq!(observe["guidance"]["delegation"], status["delegation"]);
    };
    live(&engine, "awaiting_acceptance", 0);
    let graph = engine.graph();
    let package = engine.read(R::DelegationPackage);
    let uuid = engine
        .store
        .read(deadline(), |s| {
            Ok(s.binding().identity().workspace_uuid().clone())
        })
        .unwrap();
    let input = json!({"schema":"podway.delegation-accept-input/v1","workspace_uuid":uuid,"session_id":graph.trace().session_id(),"session_revision":graph.trace().revision(),
        "attempt_id":graph.trace().active_attempt().unwrap().attempt_id(),"idempotency_key":"accept","package_digest":package["package_digest"],
        "execution":{"binding_ref":"local-specialist","adapter_id":"adapter","adapter_version":"1","primary_host":"external","scope_kind":"session","scope_ref":"scope-1",
            "supervisor_ref":"supervisor-1","worker_ref":"worker-1","task_ref":"task-1","role_ref":"reviewer","role_snapshot_digest":digest(),"agent_configuration_digest":digest(),"accepted_task_digest":digest()},
        "rounds_used":1,"result_digest":digest(),"summary":"Reviewed archived result","worker_disposition":"idle","accepted":true});
    engine.success(
        D::DelegationAccept(DelegationAcceptV1 {
            input,
            preconditions: engine.fences(),
        }),
        "accept",
    );
    live(&engine, "accepted", 1);
    let receipt = engine.read(R::DelegationReceipt {
        session_id: graph.trace().session_id().clone(),
        attempt_id: graph.trace().active_attempt().unwrap().attempt_id().clone(),
    });
    cancel(&engine, "cancel");
    engine.success(disposition(&engine), "disposition");
    engine.success(
        D::Session(S::Archive(SessionArchiveV1 {
            preconditions: identity(&engine),
        })),
        "archive",
    );
    engine.cold();
    let shown = show(&engine, graph.trace().session_id());
    assert_eq!(shown["schema"], "podway.session-archive-show-result/v3");
    assert_eq!(shown["state"]["schema"], "podway.status-result/v5");
    assert_eq!(shown["receipt_summaries_total"], 1);
    assert_eq!(shown["receipt_summaries_truncated"], false);
    let expected = podway_store::DelegationReceiptV1::new(receipt["receipt"].clone()).unwrap();
    assert_eq!(shown["receipt_summaries"], json!([expected.summary()]));
    let summaries = shown["receipt_summaries"].as_array().unwrap();
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0]["receipt_id"], receipt["receipt"]["receipt_id"]);
}
#[test]
fn state001_reset_dry_run_fence_rejection_precedes_clock_and_receipt_admission() {
    // A forged stored receipt is latent until the view's receipt stage. Reset
    // dry-run must still fail on its first fences, before the clock and the
    // view: NoClock panics if the clock is read, and the codes must be the
    // fence errors rather than WORKSPACE_STATE_UNREADABLE.
    let source = json!({"schema":"podway.procedure/v2","id":"reset-fence-delegation","version":"1","name":"Reset fence delegation","purpose":"Pin reset fence priority.",
        "node_definitions":{"work":{"type":"action","title":"Work","intent":"Review work."}},
        "graph":{"entry":"work","nodes":[{"id":"work","use":"work","terminal":true,"delegation":{"binding_ref":"local-specialist","role_ref":"reviewer","role_snapshot_digest":digest(),"max_rounds":3,"after_accept":"retain"}}]}});
    let mut engine = empty(&serde_json::to_string(&source).unwrap());
    let canonical = podway_core::canonicalize_json_v1(&source).unwrap();
    use sha2::{Digest as _, Sha256};
    let snapshot = podway_store::admit_procedure_snapshot_v2(
        podway_core::ProcedureSnapshotId::new("00000000-0000-4000-8000-000000015113").unwrap(),
        podway_core::CanonicalProcedureJsonV1::new(canonical.clone()).unwrap(),
        podway_core::Sha256Digest::new(format!(
            "sha256:{:x}",
            Sha256::digest(canonical.as_bytes())
        ))
        .unwrap(),
        podway_core::ProcedureSourceLabelV1::file("reset-fence-delegation.yaml").unwrap(),
        UnixMillis::new(2),
    )
    .unwrap();
    let running = GraphSessionStateV2::prepared(
        Revision::new(1),
        "Reset fence delegation",
        snapshot,
        SessionId::new("00000000-0000-4000-8000-000000015114").unwrap(),
        UnixMillis::new(3),
    )
    .unwrap()
    .begin_v2(
        Revision::ZERO,
        AttemptId::new("00000000-0000-4000-8000-000000015115").unwrap(),
        None,
        UnixMillis::new(4),
    )
    .unwrap()
    .into_state();
    seed(&engine, &running, "seed-reset-fences");
    let graph = engine.graph();
    let package = engine.read(R::DelegationPackage);
    let uuid = engine
        .store
        .read(deadline(), |s| {
            Ok(s.binding().identity().workspace_uuid().clone())
        })
        .unwrap();
    let input = json!({"schema":"podway.delegation-accept-input/v1","workspace_uuid":uuid,"session_id":graph.trace().session_id(),"session_revision":graph.trace().revision(),
        "attempt_id":graph.trace().active_attempt().unwrap().attempt_id(),"idempotency_key":"reset-fence-accept","package_digest":package["package_digest"],
        "execution":{"binding_ref":"local-specialist","adapter_id":"adapter","adapter_version":"1","primary_host":"external","scope_kind":"session","scope_ref":"scope-1",
            "supervisor_ref":"supervisor-1","worker_ref":"worker-1","task_ref":"task-1","role_ref":"reviewer","role_snapshot_digest":digest(),"agent_configuration_digest":digest(),"accepted_task_digest":digest()},
        "rounds_used":1,"result_digest":digest(),"summary":"Reviewed reset fences","worker_disposition":"idle","accepted":true});
    engine.success(
        D::DelegationAccept(DelegationAcceptV1 {
            input,
            preconditions: engine.fences(),
        }),
        "reset-fence-accept",
    );
    // Healthy control: the dry run succeeds before any corruption.
    engine.success(
        reset(&engine, SessionDeletionModeV1::Eligible, true),
        "reset-healthy",
    );
    // Forge the stored receipt (canonical, identity-mismatched) out of band.
    let database = engine.store.layout().database();
    let connection = rusqlite::Connection::open(&database).unwrap();
    let (attempt_id, session_id, receipt_id, receipt_json): (String, String, String, String) =
        connection
            .query_row(
                "SELECT attempt_id,session_id,receipt_id,receipt_json FROM v2_delegation_receipts",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
    drop(connection);
    let mut forged: serde_json::Value = serde_json::from_str(&receipt_json).unwrap();
    forged["workspace_uuid"] = json!("00000000-0000-4000-8000-000000015199");
    let stored = podway_store::DelegationReceiptV1::new(forged)
        .unwrap()
        .canonical_json()
        .to_owned();
    let connection = rusqlite::Connection::open(&database).unwrap();
    connection
        .execute("DELETE FROM v2_delegation_receipts", [])
        .unwrap();
    connection
        .execute(
            "INSERT INTO v2_delegation_receipts(attempt_id,session_id,receipt_id,receipt_json) VALUES (?1,?2,?3,?4)",
            rusqlite::params![attempt_id, session_id, receipt_id, stored],
        )
        .unwrap();
    drop(connection);
    // Positive control: with matching fences and an ordinary clock the same
    // dry run reaches the view and the forged receipt is actually refused
    // there, so the fence rejections below are order evidence, not the only
    // signal that the corruption is live.
    reject(
        &engine,
        reset(&engine, SessionDeletionModeV1::Eligible, true),
        "reset-corrupt-receipt",
        "WORKSPACE_STATE_UNREADABLE",
    );
    let base = identity(&engine);
    engine.reject(
        D::Session(S::Reset(SessionResetV1 {
            mode: SessionDeletionModeV1::Eligible,
            dry_run: true,
            preconditions: SessionIdentityPreconditionsWireV1 {
                expected_session_revision: Revision::new(base.expected_session_revision.get() + 1),
                ..base.clone()
            },
        })),
        "reset-stale-revision",
        "SESSION_REVISION_CONFLICT",
        &NoClock,
    );
    engine.reject(
        D::Session(S::Reset(SessionResetV1 {
            mode: SessionDeletionModeV1::Eligible,
            dry_run: true,
            preconditions: SessionIdentityPreconditionsWireV1 {
                expected_session_id: SessionId::new("00000000-0000-4000-8000-000000015198")
                    .unwrap(),
                ..identity(&engine)
            },
        })),
        "reset-wrong-session",
        "SESSION_ID_MISMATCH",
        &NoClock,
    );
}
#[test]
fn v2run007_preadmission_fences_reject_without_jobs_and_exact_replay_wins() {
    let engine = Engine::new(ACTION_READBACK_PROCEDURE);
    let before = engine.fences();
    let block = |preconditions| {
        D::Cursor(C::Block(SessionBlockV1 {
            reason: "Fence test".into(),
            preconditions,
        }))
    };
    let wrong = SessionId::new("00000000-0000-4000-8000-000000007999").unwrap();
    let stale_attempt = AttemptId::new("00000000-0000-4000-8000-000000007998").unwrap();
    reject(
        &engine,
        block(SessionMutationPreconditionsWireV1 {
            expected_session_id: wrong,
            ..before.clone()
        }),
        "wrong-session",
        "SESSION_ID_MISMATCH",
    );
    reject(
        &engine,
        block(SessionMutationPreconditionsWireV1 {
            expected_session_revision: Revision::new(before.expected_session_revision.get() + 1),
            ..before.clone()
        }),
        "wrong-revision",
        "SESSION_REVISION_CONFLICT",
    );
    reject(
        &engine,
        block(SessionMutationPreconditionsWireV1 {
            expected_attempt_id: stale_attempt,
            ..before.clone()
        }),
        "wrong-attempt",
        "ATTEMPT_NOT_CURRENT",
    );
    let summary = item_fences(&engine, "summary");
    let note = item_fences(&engine, "internal-note");
    let set = |id: &str, value: &str, preconditions| {
        D::Item(I::Set(ItemSetV1 {
            item_id: ItemId::new(id).unwrap(),
            value: value.into(),
            preconditions,
        }))
    };
    reject(
        &engine,
        set(
            "summary",
            "stale",
            ItemMutationPreconditionsWireV1 {
                expected_item_revision: Revision::new(1),
                ..summary.clone()
            },
        ),
        "wrong-item-revision",
        "ITEM_REVISION_CONFLICT",
    );
    reject(
        &engine,
        set("missing", "not found", summary.clone()),
        "missing-item",
        "ITEM_NOT_FOUND",
    );
    let request = engine.request(set("summary", "first", summary.clone()), "summary");
    let once = engine.output(engine.execute(request.clone(), &Clock));
    engine.success(
        set("internal-note", "independent item change", note),
        "note",
    );
    assert_eq!(engine.output(engine.execute(request, &NoClock)), once);
    reject(
        &engine,
        set("summary", "drift", summary.clone()),
        "summary",
        "IDEMPOTENCY_KEY_REUSED",
    );
    reject(
        &engine,
        set("summary", "new stale", summary),
        "stale-item",
        "ITEM_REVISION_CONFLICT",
    );
    reject(
        &engine,
        block(before.clone()),
        "stale-session",
        "SESSION_REVISION_CONFLICT",
    );
    let graph = engine.graph();
    assert_eq!(
        graph.trace().revision().get(),
        before.expected_session_revision.get() + 2
    );
    assert_eq!(
        item_fences(&engine, "summary").expected_item_revision,
        Revision::new(1)
    );
    assert_eq!(
        item_fences(&engine, "internal-note").expected_item_revision,
        Revision::new(1)
    );
}
#[test]
fn v2run006_reset_dry_run_result_accepts_a_not_admitted_projection() {
    // Retain the original pure closed-result contract check with the direct schema generation.
    let result = json!({"schema":"podway.session-reset-result/v2","dry_run":true,"mode":"eligible","session_id":"00000000-0000-4000-8000-000000006000","session_revision":1,
        "lifecycle":"running","current_terminal_disposition":false,"eligible":false,"required_action":"force","reset":false});
    assert!(validate_daemon_free_document_v1(
        "podway.session-reset-result/v2",
        &result
    ));
    assert!(result.get("admission").is_none());
}
#[test]
fn v2run006_blocked_state_unblocks_restarts_completes_and_resets() {
    let mut engine = Engine::new(STATE_PROCEDURE);
    engine.success(item(&engine, "check", "ready", ""), "ready");
    let request = engine.request(
        D::Cursor(C::Block(SessionBlockV1 {
            reason: "Waiting for the dependency.".into(),
            preconditions: engine.fences(),
        })),
        "block",
    );
    let once = engine.output(engine.execute(request.clone(), &Clock));
    assert_eq!(once["result"]["transition"], "block");
    assert_eq!(once["result"]["from_graph_node_id"], "work");
    assert_eq!(once["result"]["reason"], "Waiting for the dependency.");
    assert_eq!(once["result"]["session_state"], "running");
    assert_eq!(
        engine.output(engine.execute(request.clone(), &NoClock)),
        once
    );
    let blocked = status(&engine);
    assert_eq!(blocked["current"]["readiness"]["items_satisfied"], true);
    assert_eq!(blocked["current"]["readiness"]["unblocked"], false);
    assert_eq!(blocked["current"]["readiness"]["can_advance"], false);
    assert_eq!(blocked["current"]["blockers_total"], 1);
    let next = engine.read(R::Next);
    let actions = next["allowed_actions"].as_array().unwrap();
    assert!(!actions.contains(&json!("session.complete")));
    assert!(actions.contains(&json!("session.skip")));
    std::fs::remove_file(engine.temporary.path().join("source.yaml")).unwrap();
    engine.cold();
    assert_eq!(engine.output(engine.execute(request, &NoClock)), once);
    assert_eq!(status(&engine)["current"]["blockers_total"], 1);
    let blocker =
        podway_core::BlockerId::new(once["result"]["blocker_id"].as_str().unwrap()).unwrap();
    let result = engine.success(
        D::Cursor(C::Unblock(SessionUnblockV1 {
            blocker_id: Some(blocker.clone()),
            all: false,
            preconditions: engine.fences(),
        })),
        "unblock",
    );
    assert_eq!(result["result"]["transition"], "unblock");
    assert_eq!(result["result"]["blocker_id"], json!(blocker));
    assert_eq!(result["result"]["session_state"], "running");
    assert_eq!(status(&engine)["current"]["readiness"]["unblocked"], true);
    assert_eq!(status(&engine)["current"]["readiness"]["can_advance"], true);
    assert_eq!(
        engine.complete("work")["result"]["to_graph_node_id"],
        "finish"
    );
    assert_eq!(
        engine.complete("finish")["result"]["session_state"],
        "completed"
    );
    let terminal = status(&engine);
    assert_eq!(terminal["session"]["lifecycle"], "completed");
    assert!(terminal["current"].is_null());
    assert!(terminal.get("latest_goal_outcome").is_none());
    reject(
        &engine,
        D::Read {
            command: R::Next,
            expected_session_id: Some(engine.graph().trace().session_id().clone()),
        },
        "terminal-next",
        "SESSION_NOT_RUNNING",
    );
    let before = engine.graph();
    let sequence = engine.sequence();
    let preview = engine.success(
        reset(&engine, SessionDeletionModeV1::Eligible, true),
        "preview",
    );
    for (field, value) in [
        ("mode", json!("eligible")),
        ("lifecycle", json!("completed")),
        ("current_terminal_disposition", json!(false)),
        ("eligible", json!(false)),
        ("required_action", json!("record_disposition")),
        ("reset", json!(false)),
    ] {
        assert_eq!(preview["result"][field], value);
    }
    assert!(preview.get("job").is_none());
    assert_eq!(engine.graph(), before);
    assert_eq!(engine.sequence(), sequence);
    engine.success(disposition(&engine), "disposition");
    let identity = identity(&engine);
    let request = engine.request(
        reset(&engine, SessionDeletionModeV1::Eligible, false),
        "reset",
    );
    let once = engine.output(engine.execute(request.clone(), &Clock));
    assert_eq!(once["result"]["eligible"], true);
    assert_eq!(once["result"]["reset"], true);
    assert_eq!(engine.output(engine.execute(request, &NoClock)), once);
    assert!(current(&engine).is_none());
    for command in [
        R::Status {
            tier: GraphStatusTierV2::Standard,
            history_before: None,
        },
        R::Next,
    ] {
        let error = reject(
            &engine,
            D::Read {
                command,
                expected_session_id: Some(identity.expected_session_id.clone()),
            },
            "old-read",
            "SESSION_ID_MISMATCH",
        );
        assert_eq!(
            error["details"]["schema"],
            "podway.session-id-mismatch-details/v3"
        );
        assert_eq!(error["exit_code"], 4);
        assert_eq!(error["retryable"], false);
        assert_eq!(
            error["details"]["expected_session_id"],
            json!(identity.expected_session_id)
        );
        assert!(error["details"]["actual_session_id"].is_null());
    }
    reject(
        &engine,
        D::Read {
            command: R::Status {
                tier: GraphStatusTierV2::Standard,
                history_before: None,
            },
            expected_session_id: None,
        },
        "absent-status",
        "SESSION_NOT_FOUND",
    );
    for dry_run in [true, false] {
        reject(
            &engine,
            D::Session(S::Reset(SessionResetV1 {
                mode: SessionDeletionModeV1::Eligible,
                dry_run,
                preconditions: identity.clone(),
            })),
            "absent-reset",
            "SESSION_ID_MISMATCH",
        );
    }
    let attempt = SessionMutationPreconditionsWireV1 {
        expected_session_id: identity.expected_session_id.clone(),
        expected_session_revision: identity.expected_session_revision,
        expected_attempt_id: AttemptId::new("00000000-0000-4000-8000-000000006006").unwrap(),
    };
    let criteria = vec![GoalCriterionWireV2 {
        criterion_id: podway_core::CriterionId::new("verified").unwrap(),
        statement: "The absent mutation is rejected.".into(),
    }];
    let node = podway_core::GraphNodeId::new("work").unwrap();
    let reason = "Reject an absent session.".to_string();
    let commands = vec![
        ProcedureV2MutationCommandV1::SessionDecide(SessionDecideV2 {
            option_id: podway_core::OptionId::new("accept").unwrap(),
            reason: reason.clone(),
            actor: None,
            preconditions: attempt.clone(),
            expected_goal_revision: None,
        }),
        ProcedureV2MutationCommandV1::SessionRework(SessionReworkV2 {
            target_graph_node_id: node.clone(),
            reason: reason.clone(),
            actor: None,
            preconditions: ReworkPreconditionsWireV2 {
                expected_session_id: identity.expected_session_id.clone(),
                expected_session_revision: identity.expected_session_revision,
                expected_attempt_id: None,
            },
        }),
        ProcedureV2MutationCommandV1::GoalDefine(GoalDefineV2 {
            goal: reason.clone(),
            criteria: criteria.clone(),
            actor: None,
            preconditions: identity.clone(),
        }),
        ProcedureV2MutationCommandV1::GoalRevise(GoalReviseV2 {
            goal: reason.clone(),
            criteria,
            target_graph_node_id: node,
            reason: reason.clone(),
            actor: None,
            reactivate: false,
            preconditions: GoalRevisionPreconditionsWireV2 {
                expected_session_id: identity.expected_session_id.clone(),
                expected_session_revision: identity.expected_session_revision,
                expected_attempt_id: None,
                expected_goal_revision: 1,
            },
        }),
        ProcedureV2MutationCommandV1::GoalAssessCriterion(GoalAssessCriterionV2 {
            criterion_id: podway_core::CriterionId::new("verified").unwrap(),
            status: "satisfied".into(),
            reason,
            evidence: vec![],
            items: vec![],
            actor: None,
            preconditions: attempt,
            expected_goal_revision: 1,
        }),
    ];
    for (n, command) in commands.into_iter().enumerate() {
        reject(
            &engine,
            D::Procedure(command),
            &format!("absent-mutation-{n}"),
            "SESSION_ID_MISMATCH",
        );
    }
    engine.cold();
    reject(
        &engine,
        D::Read {
            command: R::Next,
            expected_session_id: Some(identity.expected_session_id),
        },
        "cold-absent",
        "SESSION_ID_MISMATCH",
    );
}
#[test]
fn v2run006_cancel_restarts_without_a_cursor_and_reset_clears_running_and_cancelled() {
    let mut cancelled = Engine::new(STATE_PROCEDURE);
    let running = Engine::new(STATE_PROCEDURE);
    cancelled.success(
        D::Cursor(C::Block(SessionBlockV1 {
            reason: "Still cancellable".into(),
            preconditions: cancelled.fences(),
        })),
        "block",
    );
    assert_eq!(status(&cancelled)["current"]["blockers_total"], 1);
    let request = cancelled.request(
        D::Cursor(C::Cancel(SessionCancelV1 {
            reason: "No longer needed".into(),
            preconditions: cancelled.fences(),
        })),
        "cancel",
    );
    let once = cancelled.output(cancelled.execute(request.clone(), &Clock));
    assert_eq!(once["result"]["transition"], "cancel");
    assert_eq!(once["result"]["session_state"], "cancelled");
    assert!(once["result"].get("reason").is_none());
    assert_eq!(
        cancelled.output(cancelled.execute(request.clone(), &NoClock)),
        once
    );
    std::fs::remove_file(cancelled.temporary.path().join("source.yaml")).unwrap();
    cancelled.cold();
    assert_eq!(cancelled.output(cancelled.execute(request, &NoClock)), once);
    let value = status(&cancelled);
    assert_eq!(value["session"]["lifecycle"], "cancelled");
    assert!(value["current"].is_null());
    let abandoned = &value["stale_attempt_history"]["entries"][0];
    assert_eq!(abandoned["lifecycle"], "abandoned");
    assert_eq!(abandoned["terminal_reason"], "No longer needed");
    reject(
        &cancelled,
        D::Read {
            command: R::Next,
            expected_session_id: Some(cancelled.graph().trace().session_id().clone()),
        },
        "cancelled-next",
        "SESSION_NOT_RUNNING",
    );
    for engine in [&cancelled, &running] {
        let id = engine.graph().trace().session_id().clone();
        let reset = engine.success(
            reset(
                engine,
                SessionDeletionModeV1::Force {
                    progress_summary:
                        "The isolated state test intentionally discards this session.".into(),
                },
                false,
            ),
            "reset-force",
        );
        assert_eq!(reset["result"]["mode"], "force");
        assert_eq!(reset["result"]["reset"], true);
        assert!(current(engine).is_none());
        reject(
            engine,
            D::Read {
                command: R::Status {
                    tier: GraphStatusTierV2::Standard,
                    history_before: None,
                },
                expected_session_id: Some(id),
            },
            "old-status",
            "SESSION_ID_MISMATCH",
        );
    }
}
