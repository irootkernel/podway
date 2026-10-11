//! Actual Engine G1/G2: API-grown L, OS writers, atomic observers and serial replay.

use super::{
    int_v2dfc006_direct_execution::{Clock, digest},
    int_v2dfc006_runtime_ports::{Engine, NoClock},
};
use podway_core::{AttemptId, ItemId, Revision, SessionLifecycle, UnixMillis};
use podway_engine::{
    direct_api::{DirectCommandV1, DirectRequestV1, DirectResponseV1, execute_direct_v1},
    direct_execution::{DirectCursorCommandV1, DirectReadCommandV1, NativeDirectIdSourceV1},
    direct_items::DirectItemCommandV1,
    direct_start::DirectStartCommandV1,
    execution::{ExecutionClockV1, prepare_workspace_procedure_source_v2},
    v2_read_service::GraphStatusTierV2,
    workspace::DirectWorkspaceResolverV1,
};
use podway_git::{
    DiagnosticPathDisplayV1, GitResolverContractV1, LosslessPathV1, NativeGitResolverV1,
    WORKTREE_SELECTOR_VERSION_V1, WorktreeSelectorV1,
};
use podway_protocol::{
    ItemMutationPreconditionsWireV1, ItemRecordManyDispositionV1, ItemRecordManyOperationV1,
    ItemRecordManyV1, ItemRecordValueV1, ItemSetV1, ProcedureV2MutationCommandV1, RequestIdV1,
    Rfc3339MillisV1, SessionBeginV1, SessionCancelV1, SessionCompleteV1, SessionDeletionModeV1,
    SessionIdentityPreconditionsWireV1, SessionMutationPreconditionsWireV1, SessionStartReplaceV1,
    SessionStartSourceV1, SessionStartV1, TerminalDispositionInputV1,
    validate_daemon_free_document_v1,
};
use podway_store::{
    ActiveItemMutationRequestV2, ActiveItemMutationV2, GraphSessionStateV2, IdempotencyKeyV1,
    ValidatedWorkspaceRootV1,
    direct_store::{DirectStoreV1, DirectTransactionBoundaryV1, DirectWorkspaceLayoutV1},
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

const CHILD_ENV: &str = "PODWAY_ENGINE_SCALE_INPUT";
const CHILD_TEST: &str = "int_v2dfc006_scale::engine_scale_child";
const EVIDENCE_ENV: &str = "PODWAY_ENGINE_SCALE_EVIDENCE";
const COUNT_ENV: &str = "PODWAY_ENGINE_SCALE_DIAGNOSTIC_COUNT";
const NOW: u64 = 1_700_000_000_001;
const L_MINIMUM_BYTES: u64 = 43_384_832;
const RUN_LIMIT: Duration = Duration::from_secs(7_200);

fn deadline() -> Instant {
    Instant::now() + Duration::from_secs(5)
}

fn procedure(nodes: usize) -> String {
    let mut source = String::from(
        "schema: podway.procedure/v2\nid: engine-scale\nversion: \"1\"\nname: Engine scale\npurpose: Qualify the production Engine G1 and G2 gates.\nnode_definitions:\n  work:\n    type: action\n    title: Work\n    intent: Record bounded text.\n    items:\n",
    );
    for n in 0..16 {
        source.push_str(&format!("      - id: item-{n}\n        type: text\n        prompt: Record payload.\n        required: false\n        max_length: 65536\n"));
    }
    source.push_str("graph:\n  entry: n0\n  nodes:\n");
    for n in 0..nodes {
        source.push_str(&format!("    - id: n{n}\n      use: work\n"));
        if n + 1 == nodes {
            source.push_str("      terminal: true\n");
        } else {
            source.push_str(&format!("      next: n{}\n", n + 1));
        }
    }
    source
}

fn fences(graph: &GraphSessionStateV2) -> SessionMutationPreconditionsWireV1 {
    SessionMutationPreconditionsWireV1 {
        expected_session_id: graph.trace().session_id().clone(),
        expected_session_revision: graph.trace().revision(),
        expected_attempt_id: graph.trace().active_attempt().unwrap().attempt_id().clone(),
    }
}
fn revision(graph: &GraphSessionStateV2, item: &ItemId) -> Revision {
    let active = graph.trace().active_attempt().unwrap().attempt_id();
    graph
        .workflow_memory()
        .attempts()
        .iter()
        .find(|memory| memory.attempt_id() == active)
        .unwrap()
        .item_slots()
        .iter()
        .find(|slot| slot.item_id() == item)
        .unwrap()
        .revision()
}
fn record(graph: &GraphSessionStateV2, values: &[(usize, String)]) -> DirectCommandV1 {
    DirectCommandV1::RecordMany(ItemRecordManyV1 {
        operations: values
            .iter()
            .map(|(n, value)| {
                let item_id = ItemId::new(format!("item-{n}")).unwrap();
                ItemRecordManyOperationV1 {
                    expected_item_revision: revision(graph, &item_id),
                    item_id,
                    disposition: ItemRecordManyDispositionV1::Record {
                        record: ItemRecordValueV1::Text {
                            value: value.clone(),
                        },
                    },
                }
            })
            .collect(),
        preconditions: fences(graph),
    })
}

/// L is produced only by successful public Engine operations, never by seeded state.
fn grow_active_l(engine: &Engine) -> (u64, usize) {
    let session = engine.graph().trace().session_id().clone();
    let mut completed_nodes = 0;
    let growth_end = Instant::now() + Duration::from_secs(600);
    loop {
        let node = completed_nodes;
        for batch in 0..6 {
            assert!(
                Instant::now() < growth_end,
                "actual API L growth exceeded its bounded deadline"
            );
            let values: Vec<_> = (0..16)
                .map(|n| (n, char::from(b'a' + batch).to_string().repeat(55_000)))
                .collect();
            engine.success(
                record(&engine.graph(), &values),
                &format!("grow-{node}-{batch}"),
            );
        }
        engine.complete(&format!("grow-complete-{node}"));
        completed_nodes += 1;
        if fs::metadata(engine.store.layout().database())
            .unwrap()
            .len()
            >= L_MINIMUM_BYTES
        {
            break;
        }
        assert!(
            completed_nodes < 100,
            "bounded actual API L growth did not reach its minimum"
        );
    }
    // Closing each real connection checkpoints its WAL; qualify the main DB itself.
    let grown_bytes = fs::metadata(engine.store.layout().database())
        .unwrap()
        .len();
    assert!(
        grown_bytes >= L_MINIMUM_BYTES,
        "actual API-grown L is only {grown_bytes} bytes"
    );
    assert_eq!(engine.graph().trace().session_id(), &session);
    assert!(engine.graph().workflow_memory().attempts().len() > 1);
    (grown_bytes, completed_nodes)
}

fn grow_l(engine: &Engine, source: &str) -> (u64, u64, usize) {
    let (grown_bytes, completed_nodes) = grow_active_l(engine);
    let old_session = engine.graph().trace().session_id().clone();
    engine.success(
        DirectCommandV1::Cursor(DirectCursorCommandV1::Cancel(SessionCancelV1 {
            reason: "Preserve the grown history and measure a fresh same-Procedure session.".into(),
            preconditions: engine.fences(),
        })),
        "grow-cancel",
    );
    let prior = engine.graph();
    engine.success(
        DirectCommandV1::Start(DirectStartCommandV1::Replace(SessionStartReplaceV1 {
            start: SessionStartV1 {
                source: SessionStartSourceV1::Procedure {
                    procedure: "source.yaml".into(),
                },
                expected_procedure_digest: Some(
                    prepare_workspace_procedure_source_v2("source.yaml", source.as_bytes())
                        .unwrap()
                        .digest()
                        .clone(),
                ),
                task_title: "Actual Engine G1/G2 fresh L session".into(),
                dry_run: false,
            },
            mode: SessionDeletionModeV1::DisposeAndArchive {
                disposition: TerminalDispositionInputV1::NotRequired {
                    reason: "The completed growth fixture is retained for Engine qualification."
                        .into(),
                },
                actor: None,
            },
            preconditions: SessionIdentityPreconditionsWireV1 {
                expected_session_id: prior.trace().session_id().clone(),
                expected_session_revision: prior.trace().revision(),
            },
        })),
        "scale-start",
    );
    let prepared = engine.graph();
    engine.success(
        DirectCommandV1::Procedure(ProcedureV2MutationCommandV1::SessionBegin(SessionBeginV1 {
            initial_goal: None,
            preconditions: SessionIdentityPreconditionsWireV1 {
                expected_session_id: prepared.trace().session_id().clone(),
                expected_session_revision: Revision::ZERO,
            },
        })),
        "scale-begin",
    );
    assert_ne!(engine.graph().trace().session_id(), &old_session);
    assert_eq!(
        engine
            .graph()
            .trace()
            .active_attempt()
            .unwrap()
            .graph_node_id()
            .as_str(),
        "n0"
    );
    engine
        .store
        .read(deadline(), |snapshot| {
            assert!(snapshot.archived_graph(&old_session)?.is_some());
            Ok(())
        })
        .unwrap();
    let fresh_bytes = fs::metadata(engine.store.layout().database())
        .unwrap()
        .len();
    assert!(
        fresh_bytes >= L_MINIMUM_BYTES,
        "fresh session shrank the grown fixture"
    );
    (grown_bytes, fresh_bytes, completed_nodes)
}

struct CountClock(AtomicU64);
impl ExecutionClockV1 for CountClock {
    fn now(&self) -> UnixMillis {
        self.0.fetch_add(1, Ordering::SeqCst);
        UnixMillis::new(NOW)
    }
}

struct Harness {
    selector: WorktreeSelectorV1,
    resolver: DirectWorkspaceResolverV1<NativeGitResolverV1>,
    store: DirectStoreV1,
}
impl Harness {
    fn open(root: &Path) -> Self {
        let selector = WorktreeSelectorV1::new(
            WORKTREE_SELECTOR_VERSION_V1,
            None,
            LosslessPathV1::from_raw_bytes(
                root.as_os_str().as_bytes(),
                DiagnosticPathDisplayV1::new(root.to_string_lossy()).unwrap(),
            )
            .unwrap(),
        )
        .unwrap();
        let worktree = NativeGitResolverV1::new()
            .resolve(selector.clone())
            .unwrap();
        let store = DirectStoreV1::new(
            DirectWorkspaceLayoutV1::new(ValidatedWorkspaceRootV1::from_path(root).unwrap())
                .unwrap(),
            digest(),
            worktree.identity().common_directory_fingerprint().clone(),
            worktree
                .identity()
                .worktree_administration_fingerprint()
                .clone(),
        );
        Self {
            selector,
            store,
            resolver: DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest()),
        }
    }
    fn request(&self, command: DirectCommandV1, key: &str) -> DirectRequestV1 {
        DirectRequestV1 {
            selector: self.selector.clone(),
            expected_workspace_id: None,
            request_id: RequestIdV1::new("00000000-0000-4000-8000-000000000099").unwrap(),
            command,
            idempotency_key: Some(IdempotencyKeyV1::new(key).unwrap()),
            deadline: deadline(),
            observed_at: Rfc3339MillisV1::from_unix_millis(NOW).unwrap(),
            timeout_ms: 5000,
        }
    }
    fn execute(&self, request: DirectRequestV1, clock: &impl ExecutionClockV1) -> DirectResponseV1 {
        let response =
            execute_direct_v1(&self.resolver, request, clock, &NativeDirectIdSourceV1).unwrap();
        match &response {
            DirectResponseV1::Success(output) => assert!(validate_daemon_free_document_v1(
                "podway.output/v4",
                output.as_value()
            )),
            DirectResponseV1::Failure(error) => assert!(validate_daemon_free_document_v1(
                "podway.error/v2",
                &error.clone().into_value()
            )),
        }
        response
    }
    fn graph(&self) -> GraphSessionStateV2 {
        self.store
            .prepare_graph(deadline())
            .unwrap()
            .unwrap()
            .graph()
            .clone()
    }
}

fn payload(writer: u64, index: u64, n: usize) -> String {
    let mut text = format!("w{writer}-i{index:06}-o{n}-{:0>48}", "");
    text.truncate(64);
    while text.len() < 64 {
        text.push('x');
    }
    text
}

fn operation(graph: &GraphSessionStateV2, writer: u64, index: u64) -> (DirectCommandV1, Value) {
    let choice = index % 50;
    let first = ((writer * 3 + index) % 16) as usize;
    if choice < 39 {
        let item = ItemId::new(format!("item-{first}")).unwrap();
        let value = payload(writer, index, first);
        let input = ItemSetV1 {
            item_id: item.clone(),
            value: value.clone(),
            preconditions: ItemMutationPreconditionsWireV1 {
                expected_session_id: graph.trace().session_id().clone(),
                expected_attempt_id: fences(graph).expected_attempt_id,
                expected_item_revision: revision(graph, &item),
            },
        };
        (
            DirectCommandV1::Item(DirectItemCommandV1::Set(input)),
            json!({"kind":"set","item":item,"value":value,"revision":revision(graph,&item),"fences":fences(graph)}),
        )
    } else if choice < 49 {
        let values: Vec<_> = (0..4)
            .map(|offset| {
                let n = (first + offset) % 16;
                (n, payload(writer, index, n))
            })
            .collect();
        let command = record(graph, &values);
        let DirectCommandV1::RecordMany(input) = &command else {
            unreachable!()
        };
        (
            command.clone(),
            json!({"kind":"record_many","operations":input.operations,"fences":input.preconditions}),
        )
    } else {
        let preconditions = fences(graph);
        (
            DirectCommandV1::Cursor(DirectCursorCommandV1::Complete(SessionCompleteV1 {
                preconditions: preconditions.clone(),
            })),
            json!({"kind":"complete","fences":preconditions}),
        )
    }
}

type Events = Arc<Mutex<Vec<(DirectTransactionBoundaryV1, Instant)>>>;
fn measured(
    harness: &Harness,
    request: DirectRequestV1,
    clock: &impl ExecutionClockV1,
    events: &Events,
    samples: &mut Vec<Value>,
    kind: &str,
) -> DirectResponseV1 {
    events.lock().unwrap().clear();
    let response = harness.execute(request, clock);
    let finished = Instant::now();
    let events = events.lock().unwrap();
    assert!(
        events.len() >= 2,
        "mutation did not enter its actual Store transaction"
    );
    assert_eq!(events[0].0, DirectTransactionBoundaryV1::BeforeBegin);
    assert_eq!(events[1].0, DirectTransactionBoundaryV1::AfterBegin);
    let committed = events
        .iter()
        .find(|(event, _)| *event == DirectTransactionBoundaryV1::AfterCommitBeforeOutput);
    let end = committed.map_or(finished, |(_, instant)| *instant);
    samples.push(json!({"kind":kind,"wait_us":events[1].1.duration_since(events[0].1).as_micros(),
        "hold_us":end.duration_since(events[1].1).as_micros(),"success":matches!(response,DirectResponseV1::Success(_))}));
    assert!(samples.len() <= 200_000, "bounded attempt log exhausted");
    response
}
fn append(file: &mut fs::File, value: &Value) {
    serde_json::to_writer(&mut *file, value).unwrap();
    file.write_all(b"\n").unwrap();
}
fn wait_file(path: &Path, end: Instant) {
    while !path.exists() {
        assert!(Instant::now() < end, "child barrier expired: {path:?}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn assert_latest_effect(graph: &GraphSessionStateV2, output: &Value) {
    let result = &output["result"];
    match output["command"].as_str().unwrap() {
        "item.set" | "item.record_many" => {
            let request_parts: Vec<_> = output["request_id"].as_str().unwrap().split('-').collect();
            let writer = u64::from_str_radix(request_parts[3], 16).unwrap() & 0xfff;
            let index = u64::from_str_radix(request_parts[4], 16).unwrap();
            let memory = graph
                .workflow_memory()
                .attempts()
                .iter()
                .find(|memory| {
                    memory.attempt_id().as_str() == result["attempt_id"].as_str().unwrap()
                })
                .unwrap();
            let items = if output["command"] == "item.set" {
                vec![result]
            } else {
                result["items"].as_array().unwrap().iter().collect()
            };
            for item in items {
                let slot = memory
                    .item_slots()
                    .iter()
                    .find(|slot| slot.item_id().as_str() == item["item_id"].as_str().unwrap())
                    .unwrap();
                if let Some(expected) = item["item_revision"].as_u64() {
                    assert_eq!(
                        slot.revision().get(),
                        expected,
                        "partial item revision publication"
                    );
                }
                let n = item["item_id"]
                    .as_str()
                    .unwrap()
                    .strip_prefix("item-")
                    .unwrap()
                    .parse::<usize>()
                    .unwrap();
                assert_eq!(
                    slot.value().unwrap().as_text().unwrap(),
                    payload(writer, index, n),
                    "partial item value publication"
                );
            }
        }
        "session.complete" => {
            assert_eq!(
                graph
                    .trace()
                    .active_attempt()
                    .unwrap()
                    .attempt_id()
                    .as_str(),
                result["to_attempt_id"].as_str().unwrap(),
                "partial cursor publication"
            );
        }
        "session.begin" => {}
        other => panic!("unexpected observed command {other}"),
    }
}

#[test]
fn engine_scale_child() {
    let Some(path) = std::env::var_os(CHILD_ENV) else {
        return;
    };
    let input: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    let path = |name: &str| PathBuf::from(input[name].as_str().unwrap());
    let mut harness = Harness::open(&path("root"));
    let initial = harness.graph();
    let base_sequence = harness
        .store
        .read(deadline(), |snapshot| snapshot.committed_sequence())
        .unwrap();
    // Warm the actual Output4 and Error2 schema validators before measured samples.
    let warm = harness.request(
        DirectCommandV1::Read {
            command: DirectReadCommandV1::Status {
                tier: GraphStatusTierV2::Standard,
                history_before: None,
            },
            expected_session_id: Some(initial.trace().session_id().clone()),
        },
        "warm-read",
    );
    assert!(matches!(
        harness.execute(warm, &Clock),
        DirectResponseV1::Success(_)
    ));
    let (mut bad, _) = operation(&initial, input["writer"].as_u64().unwrap_or(0), 0);
    let DirectCommandV1::Item(DirectItemCommandV1::Set(item)) = &mut bad else {
        unreachable!()
    };
    item.preconditions.expected_item_revision = Revision::new(u64::MAX);
    assert!(matches!(
        harness.execute(
            harness.request(bad, &format!("warm-error-{}", input["writer"])),
            &NoClock
        ),
        DirectResponseV1::Failure(_)
    ));
    fs::write(path("ready"), b"ready").unwrap();
    wait_file(&path("start"), Instant::now() + Duration::from_secs(60));
    let started = Instant::now();
    let end = started + RUN_LIMIT;
    if input["role"] == "observer" {
        let mut reads = 0;
        while !path("stop").exists() {
            assert!(Instant::now() < end, "observer deadline expired");
            harness
                .store
                .read(deadline(), |snapshot| {
                    let view = snapshot.view(UnixMillis::new(NOW))?;
                    let graph = view.graph().unwrap();
                    let changes = view.sequence() - base_sequence;
                    assert_eq!(graph.trace().session_id(), initial.trace().session_id());
                    assert_eq!(
                        graph.trace().revision().get(),
                        initial.trace().revision().get() + changes
                    );
                    assert_eq!(
                        graph.workspace_revision().get(),
                        initial.workspace_revision().get() + changes
                    );
                    let latest = snapshot.latest_outcome()?.unwrap();
                    assert_eq!(
                        latest.sequence(),
                        view.sequence(),
                        "partial state/outcome publication"
                    );
                    assert_latest_effect(
                        graph,
                        &serde_json::from_str(latest.response().as_str()).unwrap(),
                    );
                    assert_eq!(graph.trace().lifecycle(), SessionLifecycle::Running);
                    assert_eq!(
                        graph.trace().attempts().len(),
                        graph.workflow_memory().attempts().len()
                    );
                    assert_eq!(
                        graph
                            .trace()
                            .attempts()
                            .iter()
                            .filter(|attempt| attempt.is_active())
                            .count(),
                        1
                    );
                    Ok(())
                })
                .unwrap();
            reads += 1;
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(reads > 0);
        fs::write(
            path("output"),
            serde_json::to_vec_pretty(
                &json!({"reads":reads,"elapsed_us":started.elapsed().as_micros()}),
            )
            .unwrap(),
        )
        .unwrap();
        return;
    }
    let writer = input["writer"].as_u64().unwrap();
    let count = input["count"].as_u64().unwrap();
    let events: Events = Arc::new(Mutex::new(Vec::with_capacity(4)));
    let observed = Arc::clone(&events);
    harness.resolver = harness
        .resolver
        .with_store_boundary_observer_v1(move |event| {
            observed.lock().unwrap().push((event, Instant::now()));
        });
    let clock = CountClock(AtomicU64::new(0));
    let mut samples = Vec::new();
    let mut outcomes = fs::File::create(path("outcomes")).unwrap();
    let mut failures = 0_u64;
    let mut counts = BTreeMap::<String, u64>::new();
    for index in 0..count {
        let sample_start = samples.len();
        loop {
            assert!(Instant::now() < end, "writer {writer} deadline expired");
            let graph = harness.graph();
            let (command, semantic) = operation(&graph, writer, index);
            let key = format!("scale-w{writer}-i{index}-try{failures}");
            let mut request = harness.request(command, &key);
            request.request_id =
                RequestIdV1::new(format!("00000000-0000-4000-8{writer:03x}-{index:012x}")).unwrap();
            let before_clock = clock.0.load(Ordering::SeqCst);
            match measured(
                &harness,
                request.clone(),
                &clock,
                &events,
                &mut samples,
                "attempt",
            ) {
                DirectResponseV1::Success(output) => {
                    assert_eq!(clock.0.load(Ordering::SeqCst), before_clock + 1);
                    if semantic["kind"] != "set" {
                        assert_eq!(
                            output.as_value()["session"]["revision_after"].as_u64(),
                            Some(graph.trace().revision().get() + 1),
                            "accepted a stale session fence"
                        );
                    }
                    let replay = measured(
                        &harness,
                        DirectRequestV1 {
                            deadline: deadline(),
                            ..request
                        },
                        &NoClock,
                        &events,
                        &mut samples,
                        "replay",
                    );
                    let DirectResponseV1::Success(replay) = replay else {
                        panic!("same-key replay rejected")
                    };
                    assert_eq!(
                        replay.as_value(),
                        output.as_value(),
                        "full same-key replay changed"
                    );
                    assert_eq!(clock.0.load(Ordering::SeqCst), before_clock + 1);
                    append(
                        &mut outcomes,
                        &json!({"key":key,"semantic":semantic,"output":output.as_value()}),
                    );
                    *counts
                        .entry(semantic["kind"].as_str().unwrap().to_owned())
                        .or_default() += 1;
                    break;
                }
                DirectResponseV1::Failure(error) => {
                    let error = error.into_value();
                    assert!(
                        matches!(
                            error["code"].as_str(),
                            Some(
                                "ITEM_REVISION_CONFLICT"
                                    | "SESSION_REVISION_CONFLICT"
                                    | "ATTEMPT_NOT_CURRENT"
                            )
                        ),
                        "unexpected caller error (including SQLITE_BUSY): {error}"
                    );
                    assert_eq!(
                        clock.0.load(Ordering::SeqCst),
                        before_clock,
                        "rejection stamped the clock"
                    );
                    harness
                        .store
                        .read(deadline(), |snapshot| {
                            assert!(
                                snapshot
                                    .outcome(&IdempotencyKeyV1::new(&key).unwrap())?
                                    .is_none(),
                                "rejected mutation retained an outcome"
                            );
                            Ok(())
                        })
                        .unwrap();
                    failures += 1;
                }
            }
        }
        for sample in &mut samples[sample_start..] {
            sample["warmup"] = json!(index < 20);
        }
    }
    outcomes.sync_all().unwrap();
    assert_eq!(clock.0.load(Ordering::SeqCst), count);
    fs::write(path("samples"), serde_json::to_vec(&samples).unwrap()).unwrap();
    fs::write(path("output"), serde_json::to_vec_pretty(&json!({"successes":count,"replays":count,"rejections":failures,
        "clocks":clock.0.load(Ordering::SeqCst),"counts":counts,"elapsed_us":started.elapsed().as_micros()})).unwrap()).unwrap();
}

struct OwnedChild {
    child: Child,
    output: PathBuf,
    log: PathBuf,
}
impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
impl OwnedChild {
    fn spawn(binary: &Path, input: &Path, output: PathBuf, log: PathBuf) -> Self {
        let stream = fs::File::create(&log).unwrap();
        let child = Command::new(binary)
            .args(["--exact", CHILD_TEST, "--nocapture", "--test-threads=1"])
            .env(CHILD_ENV, input)
            .stdin(Stdio::null())
            .stdout(stream.try_clone().unwrap())
            .stderr(stream)
            .spawn()
            .unwrap();
        Self { child, output, log }
    }
}
fn percentile(values: &mut [u64], n: usize) -> u64 {
    values.sort_unstable();
    values[(values.len() * n).div_ceil(100).saturating_sub(1)]
}

fn serial_apply(mut state: GraphSessionStateV2, record: &Value) -> GraphSessionStateV2 {
    let input = &record["semantic"];
    let session_fences = SessionMutationPreconditionsWireV1 {
        expected_session_id: podway_core::SessionId::new(
            input["fences"]["expected_session_id"].as_str().unwrap(),
        )
        .unwrap(),
        expected_session_revision: Revision::new(
            input["fences"]["expected_session_revision"]
                .as_u64()
                .unwrap(),
        ),
        expected_attempt_id: AttemptId::new(
            input["fences"]["expected_attempt_id"].as_str().unwrap(),
        )
        .unwrap(),
    };
    assert_eq!(
        session_fences.expected_session_id,
        *state.trace().session_id()
    );
    assert_eq!(
        session_fences.expected_attempt_id,
        *state.trace().active_attempt().unwrap().attempt_id()
    );
    let now = UnixMillis::new(NOW);
    match input["kind"].as_str().unwrap() {
        "set" => {
            state = state
                .mutate_active_item_v2(
                    &session_fences.expected_attempt_id,
                    &ItemId::new(input["item"].as_str().unwrap()).unwrap(),
                    Revision::new(input["revision"].as_u64().unwrap()),
                    ActiveItemMutationV2::Set {
                        value: input["value"].as_str().unwrap().into(),
                    },
                    now,
                )
                .unwrap()
                .into_state();
        }
        "record_many" => {
            assert_eq!(
                session_fences.expected_session_revision,
                state.trace().revision(),
                "stale-fence batch committed"
            );
            let mut mutations: Vec<_> = input["operations"]
                .as_array()
                .unwrap()
                .iter()
                .map(|operation| {
                    ActiveItemMutationRequestV2::new(
                        ItemId::new(operation["item_id"].as_str().unwrap()).unwrap(),
                        Revision::new(operation["expected_item_revision"].as_u64().unwrap()),
                        ActiveItemMutationV2::Set {
                            value: operation["record"]["value"].as_str().unwrap().into(),
                        },
                    )
                })
                .collect();
            mutations.sort_by(|left, right| left.item_id().cmp(right.item_id()));
            state = state
                .mutate_active_items_v2(&session_fences.expected_attempt_id, &mutations, now)
                .unwrap()
                .into_state();
        }
        "complete" => {
            assert_eq!(
                session_fences.expected_session_revision,
                state.trace().revision(),
                "stale-fence completion committed"
            );
            let fresh = record["output"]["result"]["to_attempt_id"]
                .as_str()
                .map(|id| AttemptId::new(id).unwrap());
            state = state
                .complete_active_action_v2(
                    session_fences.expected_session_revision,
                    &session_fences.expected_attempt_id,
                    fresh,
                    now,
                )
                .unwrap()
                .into_state();
        }
        other => panic!("unknown accepted command {other}"),
    }
    assert_eq!(
        record["output"]["session"]["revision_after"].as_u64(),
        Some(state.trace().revision().get())
    );
    state
}

/// Shared G5 fixture: actual API-grown L retained as an archive, fresh active n0.
pub(super) fn grown_l_engine(nodes: usize) -> (Engine, (u64, u64, usize)) {
    let source = procedure(nodes);
    let engine = Engine::new(&source);
    let metrics = grow_l(&engine, &source);
    (engine, metrics)
}

fn qualify(nodes: usize) {
    if cfg!(debug_assertions) {
        panic!("G1/G2 qualification requires the release profile");
    }
    // Cargo may rebuild this test target while the fixture grows. Every child
    // executes the same immutable test-binary copy captured before that growth.
    let executable_directory = tempfile::Builder::new()
        .prefix("v2dfc006-scale-bin-")
        .tempdir()
        .unwrap();
    let executable = executable_directory.path().join("engine-scale-tests");
    fs::copy(std::env::current_exe().unwrap(), &executable).unwrap();
    let executable_sha256 = format!(
        "sha256:{:x}",
        Sha256::digest(fs::read(&executable).unwrap())
    );
    let engine = Engine::new(&procedure(nodes));
    let (grown_bytes, growth_nodes) = grow_active_l(&engine);
    let initial = engine.graph();
    let initial_sequence = engine.sequence();
    let diagnostic = std::env::var(COUNT_ENV)
        .ok()
        .map(|value| value.parse::<u64>().unwrap());
    let count = diagnostic.unwrap_or(1000);
    assert!(
        count > 0 && count <= 1000 && count.is_multiple_of(50),
        "diagnostic count must be a positive multiple of 50 <=1000"
    );
    let retained = std::env::var_os(EVIDENCE_ENV).map(PathBuf::from);
    let directory = retained
        .unwrap_or_else(|| engine.temporary.path().join("scale-evidence"))
        .join(format!("g1-g2-{nodes}"));
    assert!(
        !directory.exists(),
        "evidence destination must be fresh: {directory:?}"
    );
    fs::create_dir_all(&directory).unwrap();
    let root = fs::canonicalize(engine.temporary.path()).unwrap();
    let start = directory.join("start");
    let stop = directory.join("stop");
    let mut children = Vec::new();
    let mut readiness = Vec::new();
    for n in 0..10 {
        let input = directory.join(format!("input-{n}.json"));
        let ready = directory.join(format!("ready-{n}"));
        let output = directory.join(format!("worker-{n}.json"));
        fs::write(&input, serde_json::to_vec(&json!({"root":root,"role":if n<8 {"writer"}else{"observer"},
            "writer":n,"count":count,"ready":ready,"start":start,"stop":stop,"output":output,
            "samples":directory.join(format!("samples-{n}.json")),"outcomes":directory.join(format!("outcomes-{n}.jsonl"))})).unwrap()).unwrap();
        children.push(OwnedChild::spawn(
            &executable,
            &input,
            output,
            directory.join(format!("worker-{n}.log")),
        ));
        readiness.push(ready);
    }
    let ready_end = Instant::now() + Duration::from_secs(60);
    while !readiness.iter().all(|path| path.exists()) {
        for child in &mut children {
            assert!(
                child.child.try_wait().unwrap().is_none(),
                "child failed before barrier: {:?}",
                child.log
            );
        }
        assert!(Instant::now() < ready_end, "readiness deadline expired");
        std::thread::sleep(Duration::from_millis(2));
    }
    let started = Instant::now();
    let end = started + RUN_LIMIT;
    fs::write(&start, b"start").unwrap();
    let mut done = [false; 10];
    let mut last_progress = Instant::now();
    while !done[..8].iter().all(|finished| *finished) {
        assert!(Instant::now() < end, "qualification deadline expired");
        for (n, child) in children.iter_mut().enumerate() {
            if done[n] {
                continue;
            }
            if let Some(status) = child.child.try_wait().unwrap() {
                assert!(
                    n < 8 && status.success(),
                    "child stopped prematurely or failed: {:?}",
                    child.log
                );
                done[n] = true;
            }
        }
        if last_progress.elapsed() >= Duration::from_secs(30) {
            println!(
                "Engine G1/G2 nodes={nodes}: {}/8 writers finished after {}s; evidence={}",
                done[..8].iter().filter(|done| **done).count(),
                started.elapsed().as_secs(),
                directory.display()
            );
            last_progress = Instant::now();
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    fs::write(&stop, b"stop").unwrap();
    let observer_end = Instant::now() + Duration::from_secs(10);
    while !done[8..].iter().all(|finished| *finished) {
        assert!(
            Instant::now() < observer_end,
            "observer shutdown deadline expired"
        );
        for n in 8..10 {
            if !done[n]
                && let Some(status) = children[n].child.try_wait().unwrap()
            {
                assert!(status.success(), "observer failed: {:?}", children[n].log);
                done[n] = true;
            }
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    let mut accepted = BTreeMap::new();
    let mut keys = BTreeSet::new();
    let mut waits = Vec::new();
    let mut holds = Vec::new();
    let mut elapsed = Vec::new();
    let mut reports = Vec::new();
    let mut transactions = BTreeMap::<String, u64>::new();
    for (n, child) in children.iter().enumerate() {
        let report: Value = serde_json::from_slice(&fs::read(&child.output).unwrap()).unwrap();
        if n < 8 {
            assert_eq!(report["successes"].as_u64(), Some(count));
            assert_eq!(report["replays"].as_u64(), Some(count));
            assert_eq!(report["clocks"].as_u64(), Some(count));
            assert_eq!(report["counts"]["set"].as_u64(), Some(count / 50 * 39));
            assert_eq!(
                report["counts"]["record_many"].as_u64(),
                Some(count / 50 * 10)
            );
            assert_eq!(report["counts"]["complete"].as_u64(), Some(count / 50));
            elapsed.push(report["elapsed_us"].as_u64().unwrap());
            let samples: Vec<Value> = serde_json::from_slice(
                &fs::read(directory.join(format!("samples-{n}.json"))).unwrap(),
            )
            .unwrap();
            for sample in samples {
                if sample["warmup"] != true {
                    waits.push(sample["wait_us"].as_u64().unwrap());
                    holds.push(sample["hold_us"].as_u64().unwrap());
                }
                let kind = if sample["kind"] == "replay" {
                    "replay"
                } else if sample["success"] == true {
                    "commit"
                } else {
                    "reject"
                };
                *transactions.entry(kind.into()).or_default() += 1;
            }
            for line in BufReader::new(
                fs::File::open(directory.join(format!("outcomes-{n}.jsonl"))).unwrap(),
            )
            .lines()
            {
                let record: Value = serde_json::from_str(&line.unwrap()).unwrap();
                assert!(
                    keys.insert(record["key"].as_str().unwrap().to_owned()),
                    "duplicate acknowledged key"
                );
                let sequence = record["output"]["workspace"]["latest_workspace_sequence"]
                    .as_u64()
                    .unwrap();
                assert!(
                    accepted.insert(sequence, record).is_none(),
                    "duplicate acknowledged sequence"
                );
            }
        } else {
            assert!(report["reads"].as_u64().unwrap() > 0);
        }
        reports.push(report);
    }
    assert_eq!(accepted.len() as u64, 8 * count);
    assert_eq!(
        engine.sequence(),
        initial_sequence + 8 * count,
        "lost or unacknowledged mutation"
    );
    let mut replay = initial;
    for (offset, (sequence, record)) in accepted.iter().enumerate() {
        assert_eq!(
            *sequence,
            initial_sequence + offset as u64 + 1,
            "noncontiguous accepted outcome log"
        );
        replay = serial_apply(replay, record);
    }
    engine
        .store
        .read(deadline(), |snapshot| {
            for (sequence, record) in &accepted {
                let outcome = snapshot
                    .outcome(&IdempotencyKeyV1::new(record["key"].as_str().unwrap()).unwrap())?
                    .unwrap();
                assert_eq!(outcome.sequence(), *sequence);
                assert_eq!(outcome.recorded_at(), UnixMillis::new(NOW));
                assert_eq!(
                    serde_json::from_str::<Value>(outcome.response().as_str()).unwrap(),
                    record["output"]
                );
            }
            Ok(())
        })
        .unwrap();
    assert_eq!(
        engine.graph(),
        replay,
        "complete final state differs from serial accepted-outcome replay"
    );
    let wait_p99 = percentile(&mut waits, 99);
    let hold_p99 = percentile(&mut holds, 99);
    let wait_max = *waits.last().unwrap();
    let hold_max = *holds.last().unwrap();
    let fastest = *elapsed.iter().min().unwrap();
    let slowest = *elapsed.iter().max().unwrap();
    let summary = json!({"nodes":nodes,"profile":"release","diagnostic":diagnostic.is_some(),
        "g1_gate":diagnostic.is_none(),"g2_gate":nodes==300 && diagnostic.is_none(),"test_binary_sha256":executable_sha256,
        "commit_hold_endpoint":"AfterCommitBeforeOutput","replay_rejection_hold_endpoint":"typed_API_return_conservative_upper_bound",
        "grown_main_database_bytes":grown_bytes,"active_session_is_grown":true,"growth_completed_nodes":growth_nodes,"writers":8,"observers":2,
        "successes_per_writer":count,"acknowledged":accepted.len(),"serial_replay_matches":true,
        "lost":0,"duplicated":0,"stale_fence_commits":0,"busy_reaching_caller":0,"observer_violations":0,
        "warmup_successes_per_writer":20,"samples":waits.len(),"wait_p50_us":percentile(&mut waits,50),"wait_p95_us":percentile(&mut waits,95),
        "hold_p50_us":percentile(&mut holds,50),"hold_p95_us":percentile(&mut holds,95),
        "wait_p99_us":wait_p99,"wait_max_us":wait_max,"hold_p99_us":hold_p99,"hold_max_us":hold_max,
        "fastest_writer_us":fastest,"slowest_writer_us":slowest,"transactions":transactions,"reports":reports});
    fs::write(
        directory.join("summary.json"),
        serde_json::to_vec_pretty(&summary).unwrap(),
    )
    .unwrap();
    println!("Engine G1/G2 summary: {summary}");
    if nodes == 300 && diagnostic.is_none() {
        assert!(
            wait_p99 <= 250_000,
            "G2 wait p99 exceeds 250ms: {wait_p99}us"
        );
        assert!(
            wait_max <= 1_000_000,
            "G2 maximum wait exceeds 1s: {wait_max}us"
        );
        assert!(hold_p99 <= 50_000, "G2 hold p99 exceeds 50ms: {hold_p99}us");
        assert!(
            slowest <= fastest * 2,
            "G2 slowest writer exceeds twice fastest"
        );
    }
}

#[test]
#[ignore = "release-only G1/G2 qualification: actual L, 8 OS writers x 1000 mutations and 2 observers"]
fn engine_g1_g2_grown_l_300() {
    qualify(300);
}

#[test]
#[ignore = "release-only maximum Procedure G1 correctness and G2 information: actual L and 8000 mutations"]
fn engine_g1_g2_grown_l_1024() {
    qualify(1024);
}
