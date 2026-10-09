//! Production Engine preparation races at actual native transaction boundaries.

use std::{
    io::{BufRead, BufReader, Read, Write},
    os::unix::ffi::OsStrExt,
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use podway_core::{
    AttemptId, BlockerId, DelegationReceiptIdV1, ItemId, ProcedureSnapshotId, Revision, SessionId,
    UnixMillis, WorkspaceId,
};
use podway_engine::{
    direct_api::{DirectCommandV1, DirectRequestV1, DirectResponseV1, execute_direct_v1},
    direct_execution::{
        DirectCursorCommandV1, DirectIdSourceV1, DirectReadCommandV1, NativeDirectIdSourceV1,
    },
    direct_items::DirectItemCommandV1,
    direct_sessions::DirectSessionCommandV1,
    direct_start::DirectStartCommandV1,
    execution::{ExecutionClockV1, prepare_workspace_procedure_source_v2},
    workspace::DirectWorkspaceResolverV1,
};
use podway_git::{
    DiagnosticPathDisplayV1, LosslessPathV1, NativeGitResolverV1, WORKTREE_SELECTOR_VERSION_V1,
    WorktreeSelectorV1,
};
use podway_protocol::{
    DelegationAcceptV1, ItemAttachSourceV1, ItemAttachV1, ItemMutationPreconditionsWireV1,
    ItemRecordManyDispositionV1, ItemRecordManyOperationV1, ItemRecordManyV1, ItemRecordValueV1,
    ItemSetV1, ProcedureV2MutationCommandV1, RequestIdV1, Rfc3339MillisV1, SessionArchiveV1,
    SessionBeginV1, SessionCancelV1, SessionCompleteV1, SessionDeletionModeV1,
    SessionIdentityPreconditionsWireV1, SessionMutationPreconditionsWireV1, SessionResetV1,
    SessionStartReplaceV1, SessionStartSourceV1, SessionStartV1, SessionTerminalDispositionV1,
    TerminalDispositionInputV1, validate_daemon_free_document_v1,
};
use podway_store::{
    GraphSessionStateV2, IdempotencyKeyV1,
    direct_store::{DirectStoreV1, DirectTransactionBoundaryV1},
};
use serde::Deserialize;
use serde_json::{Value, json};

use super::int_v2dfc006_direct_execution::{Clock, deadline, digest, fixture};

const RESPONSE: &str = "00000000-0000-4000-8000-000000000088";
const CHILD_PREFIX: &str = "V2DFC006_RACE ";
const TRIALS: usize = 500;

struct NoEffects;
impl ExecutionClockV1 for NoEffects {
    fn now(&self) -> UnixMillis {
        panic!("stale preparation/replay read the clock")
    }
}
impl DirectIdSourceV1 for NoEffects {
    fn next_workspace_id(&self) -> WorkspaceId {
        panic!("unexpected workspace ID")
    }
    fn next_session_id(&self) -> SessionId {
        panic!("unexpected session ID")
    }
    fn next_attempt_id(&self) -> AttemptId {
        panic!("stale preparation/replay generated attempt ID")
    }
    fn next_blocker_id(&self) -> BlockerId {
        panic!("unexpected blocker ID")
    }
    fn next_procedure_snapshot_id(&self) -> ProcedureSnapshotId {
        panic!("unexpected snapshot ID")
    }
    fn next_delegation_receipt_id(&self) -> DelegationReceiptIdV1 {
        panic!("unexpected receipt ID")
    }
}

fn resolver() -> DirectWorkspaceResolverV1<NativeGitResolverV1> {
    DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest())
}

fn request(selector: &WorktreeSelectorV1, command: DirectCommandV1, key: &str) -> DirectRequestV1 {
    DirectRequestV1 {
        selector: selector.clone(),
        expected_workspace_id: None,
        request_id: RequestIdV1::new(RESPONSE).unwrap(),
        command,
        idempotency_key: Some(IdempotencyKeyV1::new(key).unwrap()),
        deadline: deadline(),
        observed_at: Rfc3339MillisV1::new("2023-11-14T22:13:20.001Z").unwrap(),
        timeout_ms: 5000,
    }
}

fn success(response: DirectResponseV1) -> Value {
    match response {
        DirectResponseV1::Success(output) => {
            assert!(validate_daemon_free_document_v1(
                "podway.output/v4",
                output.as_value()
            ));
            output.as_value().clone()
        }
        DirectResponseV1::Failure(error) => {
            panic!("unexpected rejection: {:?}", error.into_value())
        }
    }
}

fn rejection(response: DirectResponseV1, code: &str) {
    let DirectResponseV1::Failure(error) = response else {
        panic!("stale preparation committed")
    };
    let value = error.into_value();
    assert!(validate_daemon_free_document_v1("podway.error/v2", &value));
    assert_eq!(value["code"], code);
}

fn graph(store: &DirectStoreV1) -> GraphSessionStateV2 {
    store
        .prepare_graph(deadline())
        .unwrap()
        .unwrap()
        .graph()
        .clone()
}

fn item_fences(graph: &GraphSessionStateV2, item: &str) -> ItemMutationPreconditionsWireV1 {
    let attempt = graph.trace().active_attempt().unwrap().attempt_id();
    let memory = graph
        .workflow_memory()
        .attempts()
        .iter()
        .find(|memory| memory.attempt_id() == attempt)
        .unwrap();
    let slot = memory
        .item_slots()
        .iter()
        .find(|slot| slot.item_id().as_str() == item)
        .unwrap();
    ItemMutationPreconditionsWireV1 {
        expected_session_id: graph.trace().session_id().clone(),
        expected_attempt_id: attempt.clone(),
        expected_item_revision: slot.revision(),
    }
}

fn cursor_fences(graph: &GraphSessionStateV2) -> SessionMutationPreconditionsWireV1 {
    SessionMutationPreconditionsWireV1 {
        expected_session_id: graph.trace().session_id().clone(),
        expected_session_revision: graph.trace().revision(),
        expected_attempt_id: graph.trace().active_attempt().unwrap().attempt_id().clone(),
    }
}

fn set(graph: &GraphSessionStateV2, item: &str, value: &str) -> DirectCommandV1 {
    DirectCommandV1::Item(DirectItemCommandV1::Set(ItemSetV1 {
        item_id: ItemId::new(item).unwrap(),
        value: value.into(),
        preconditions: item_fences(graph, item),
    }))
}

fn complete(graph: &GraphSessionStateV2) -> DirectCommandV1 {
    DirectCommandV1::Cursor(DirectCursorCommandV1::Complete(SessionCompleteV1 {
        preconditions: cursor_fences(graph),
    }))
}

fn state_command(graph: &GraphSessionStateV2, trial: usize) -> DirectCommandV1 {
    match trial % 3 {
        0 => set(graph, "i01", &format!("parent-{trial}")),
        1 => DirectCommandV1::RecordMany(ItemRecordManyV1 {
            preconditions: cursor_fences(graph),
            operations: vec![ItemRecordManyOperationV1 {
                item_id: ItemId::new("i02").unwrap(),
                expected_item_revision: item_fences(graph, "i02").expected_item_revision,
                disposition: ItemRecordManyDispositionV1::Record {
                    record: ItemRecordValueV1::Text {
                        value: format!("parent-{trial}"),
                    },
                },
            }],
        }),
        _ => complete(graph),
    }
}

fn outcome(store: &DirectStoreV1, key: &str) -> Option<Value> {
    store
        .read(deadline(), |snapshot| {
            Ok(snapshot
                .outcome(&IdempotencyKeyV1::new(key).unwrap())?
                .map(|outcome| serde_json::from_str(outcome.response().as_str()).unwrap()))
        })
        .unwrap()
}

fn sequence(store: &DirectStoreV1) -> u64 {
    store
        .read(deadline(), |snapshot| snapshot.committed_sequence())
        .unwrap()
}

fn attach(selector: &WorktreeSelectorV1, store: &DirectStoreV1, key: &str) {
    let command = DirectCommandV1::Item(DirectItemCommandV1::Attach(ItemAttachV1 {
        item_id: ItemId::new("artifact").unwrap(),
        source: ItemAttachSourceV1::Path {
            path: "artifact.txt".into(),
            media_type: Some("text/plain".into()),
        },
        preconditions: item_fences(&graph(store), "artifact"),
    }));
    success(
        execute_direct_v1(
            &resolver(),
            request(selector, command, key),
            &Clock,
            &NativeDirectIdSourceV1,
        )
        .unwrap(),
    );
}

fn prepare_fixture() -> (tempfile::TempDir, WorktreeSelectorV1, DirectStoreV1) {
    let (temporary, selector, store, _) = fixture();
    let mut items: Vec<Value> = (1..=16)
        .map(|index| {
            json!({"id":format!("i{index:02}"),
        "type":"text","prompt":"Record evidence.","required":false,"max_length":65536})
        })
        .collect();
    items.push(
        json!({"id":"artifact","type":"artifact","prompt":"Attach local report.","required":true}),
    );
    let source = serde_json::to_vec(&json!({"schema":"podway.procedure/v2","id":"races","version":"1",
        "name":"Preparation races","purpose":"Reject stale native preparation.",
        "node_definitions":{"work":{"type":"action","title":"Work","intent":"Record evidence.","items":items}},
        "graph":{"entry":"first","nodes":[{"id":"first","use":"work","next":"second"},
            {"id":"second","use":"work","next":"third"},{"id":"third","use":"work","terminal":true}]}})).unwrap();
    std::fs::write(temporary.path().join("race.json"), &source).unwrap();
    std::fs::write(
        temporary.path().join("artifact.txt"),
        b"original fixture artifact\n",
    )
    .unwrap();
    let start = DirectCommandV1::Start(DirectStartCommandV1::Start(SessionStartV1 {
        source: SessionStartSourceV1::Procedure {
            procedure: "race.json".into(),
        },
        expected_procedure_digest: Some(
            prepare_workspace_procedure_source_v2("race.json", &source)
                .unwrap()
                .digest()
                .clone(),
        ),
        task_title: "Actual G6 preparation races".into(),
        dry_run: false,
    }));
    success(
        execute_direct_v1(
            &resolver(),
            request(&selector, start, "race-start"),
            &Clock,
            &NativeDirectIdSourceV1,
        )
        .unwrap(),
    );
    let begin =
        DirectCommandV1::Procedure(ProcedureV2MutationCommandV1::SessionBegin(SessionBeginV1 {
            initial_goal: None,
            preconditions: SessionIdentityPreconditionsWireV1 {
                expected_session_id: graph(&store).trace().session_id().clone(),
                expected_session_revision: Revision::ZERO,
            },
        }));
    success(
        execute_direct_v1(
            &resolver(),
            request(&selector, begin, "race-begin"),
            &Clock,
            &NativeDirectIdSourceV1,
        )
        .unwrap(),
    );
    attach(&selector, &store, "race-attach-first");
    (temporary, selector, store)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CompetingRequest {
    trial: usize,
    #[serde(default)]
    item: Option<String>,
    #[serde(default)]
    start: bool,
}

// One owned child avoids the process-local SQLite mapping gate while the parent
// is paused before BEGIN. Its pipe protocol is bounded and deterministic.
struct Competitor {
    child: Child,
    input: Option<ChildStdin>,
    output: mpsc::Receiver<Value>,
}
impl Competitor {
    fn new(root: &std::path::Path) -> Self {
        Self::with_trials(root, TRIALS)
    }
    fn with_trials(root: &std::path::Path, trials: usize) -> Self {
        assert!((1..=TRIALS).contains(&trials));
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "int_v2dfc006_races::preparation_race_child",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("PODWAY_ENGINE_G6_OWNED_FIXTURE", root)
            .env("PODWAY_ENGINE_G6_TRIALS", trials.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let input = child.stdin.take();
        let stdout = child.stdout.take().unwrap();
        let (send, output) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let line = line.unwrap();
                assert!(line.len() <= 16384);
                if let Some((_, document)) = line.split_once(CHILD_PREFIX)
                    && send.send(serde_json::from_str(document).unwrap()).is_err()
                {
                    break;
                }
            }
        });
        Self {
            child,
            input,
            output,
        }
    }
    fn commit(&mut self, trial: usize) -> Value {
        self.commit_request(json!({"trial":trial}))
    }
    fn commit_item(&mut self, trial: usize, item: &str) -> Value {
        self.commit_request(json!({"trial":trial,"item":item}))
    }
    fn start(&mut self) -> Value {
        self.commit_request(json!({"trial":0,"start":true}))
    }
    fn commit_request(&mut self, request: Value) -> Value {
        let input = self.input.as_mut().unwrap();
        writeln!(input, "{request}").unwrap();
        input.flush().unwrap();
        self.output
            .recv_timeout(Duration::from_secs(4))
            .expect("competing Engine commit missed bounded barrier")
    }
    fn finish(&mut self) {
        self.input.take();
        let limit = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            assert!(
                Instant::now() < limit,
                "owned competing child failed to finish"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}
impl Drop for Competitor {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn preparation_race_child() {
    let Some(root) = std::env::var_os("PODWAY_ENGINE_G6_OWNED_FIXTURE") else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    assert!(root.join(".git").is_dir());
    let path = LosslessPathV1::from_raw_bytes(
        root.as_os_str().as_bytes(),
        DiagnosticPathDisplayV1::new(root.to_string_lossy()).unwrap(),
    )
    .unwrap();
    let selector = WorktreeSelectorV1::new(WORKTREE_SELECTOR_VERSION_V1, None, path).unwrap();
    let (_, store) = resolver()
        .resolve_existing(selector.clone(), None, deadline())
        .unwrap();
    let mut input = BufReader::new(std::io::stdin());
    let trials: usize = std::env::var("PODWAY_ENGINE_G6_TRIALS")
        .unwrap()
        .parse()
        .unwrap();
    assert!((1..=TRIALS).contains(&trials));
    let mut count = 0;
    loop {
        let mut line = String::new();
        if (&mut input).take(4097).read_line(&mut line).unwrap() == 0 {
            break;
        }
        assert!(line.len() <= 4096 && line.ends_with('\n'));
        let CompetingRequest { trial, item, start } = serde_json::from_str(&line).unwrap();
        assert_eq!(trial, count);
        assert!(trial < trials);
        let item = item
            .as_deref()
            .unwrap_or(if trial % 3 == 0 { "i01" } else { "i16" });
        assert!(matches!(item, "i01" | "i16"));
        let key = format!("g6-competing-{trial}");
        let command = if start {
            assert_eq!((trial, trials), (0, 1));
            plain_start(&root)
        } else {
            set(&graph(&store), item, &format!("competing-{trial}"))
        };
        let input = request(&selector, command, &key);
        let output = success(
            execute_direct_v1(&resolver(), input.clone(), &Clock, &NativeDirectIdSourceV1).unwrap(),
        );
        assert_eq!(
            success(execute_direct_v1(&resolver(), input, &NoEffects, &NoEffects).unwrap()),
            output
        );
        assert_eq!(outcome(&store, &key), Some(output.clone()));
        println!("{CHILD_PREFIX}{}", serde_json::to_string(&output).unwrap());
        std::io::stdout().flush().unwrap();
        count += 1;
    }
    assert_eq!(count, trials);
}

fn plain_start(root: &std::path::Path) -> DirectCommandV1 {
    let source = std::fs::read(root.join("race.json")).unwrap();
    DirectCommandV1::Start(DirectStartCommandV1::Start(SessionStartV1 {
        source: SessionStartSourceV1::Procedure {
            procedure: "race.json".into(),
        },
        expected_procedure_digest: Some(
            prepare_workspace_procedure_source_v2("race.json", &source)
                .unwrap()
                .digest()
                .clone(),
        ),
        task_title: "Concurrent plain start".into(),
        dry_run: false,
    }))
}

#[test]
fn plain_start_race_rechecks_the_winning_session_policy_without_a_revision_hint() {
    let (temporary, selector, store) = prepare_fixture();
    let current = graph(&store);
    let cancel = DirectCommandV1::Cursor(DirectCursorCommandV1::Cancel(SessionCancelV1 {
        reason: "Finish the isolated predecessor fixture.".into(),
        preconditions: cursor_fences(&current),
    }));
    success(
        execute_direct_v1(
            &resolver(),
            request(&selector, cancel, "cancel-predecessor"),
            &Clock,
            &NativeDirectIdSourceV1,
        )
        .unwrap(),
    );
    let terminal = graph(&store);
    let disposition = DirectCommandV1::Procedure(
        ProcedureV2MutationCommandV1::SessionTerminalDisposition(SessionTerminalDispositionV1 {
            disposition: TerminalDispositionInputV1::NotRequired {
                reason: "The isolated fixture has no external handoff.".into(),
            },
            actor: None,
            preconditions: SessionIdentityPreconditionsWireV1 {
                expected_session_id: terminal.trace().session_id().clone(),
                expected_session_revision: terminal.trace().revision(),
            },
        }),
    );
    success(
        execute_direct_v1(
            &resolver(),
            request(&selector, disposition, "dispose-predecessor"),
            &Clock,
            &NativeDirectIdSourceV1,
        )
        .unwrap(),
    );
    let before_sequence = sequence(&store);
    let competitor = Arc::new(Mutex::new(Competitor::with_trials(temporary.path(), 1)));
    let instrumented = resolver().with_store_boundary_observer_v1({
        let competitor = Arc::clone(&competitor);
        move |boundary| {
            if boundary == DirectTransactionBoundaryV1::BeforeBegin {
                competitor.lock().unwrap().start();
            }
        }
    });
    let input = request(
        &selector,
        plain_start(temporary.path()),
        "losing-plain-start",
    );
    let DirectResponseV1::Failure(error) =
        execute_direct_v1(&instrumented, input, &NoEffects, &NoEffects).unwrap()
    else {
        panic!("concurrent plain start replaced the winning session");
    };
    competitor.lock().unwrap().finish();
    let error = error.into_value();
    assert!(validate_daemon_free_document_v1("podway.error/v2", &error));
    assert_eq!(error["code"], "SESSION_START_DECISION_REQUIRED");
    assert_eq!(error["details"]["lifecycle"], "prepared");
    assert_eq!(error["details"]["current_terminal_disposition"], false);
    assert_eq!(sequence(&store), before_sequence + 1);
    assert!(outcome(&store, "losing-plain-start").is_none());
    assert_ne!(
        graph(&store).trace().session_id(),
        terminal.trace().session_id()
    );
    assert_eq!(graph(&store).trace().revision(), Revision::ZERO);
    assert!(outcome(&store, "g6-competing-0").is_some());
}

#[test]
fn unrelated_item_commit_preserves_exact_item_fences_and_replay() {
    let (temporary, selector, store) = prepare_fixture();
    let before = graph(&store);
    let before_sequence = sequence(&store);
    let competitor = Arc::new(Mutex::new(Competitor::with_trials(temporary.path(), 1)));
    let instrumented = resolver().with_store_boundary_observer_v1({
        let competitor = Arc::clone(&competitor);
        move |boundary| {
            if boundary == DirectTransactionBoundaryV1::BeforeBegin {
                competitor.lock().unwrap().commit_item(0, "i16");
            }
        }
    });
    let input = request(&selector, set(&before, "i01", "parent"), "independent-item");
    let output = success(
        execute_direct_v1(
            &instrumented,
            input.clone(),
            &Clock,
            &NativeDirectIdSourceV1,
        )
        .unwrap(),
    );
    competitor.lock().unwrap().finish();
    assert_eq!(sequence(&store), before_sequence + 2);
    assert_eq!(
        item_fences(&graph(&store), "i01").expected_item_revision,
        Revision::new(1)
    );
    assert_eq!(
        item_fences(&graph(&store), "i16").expected_item_revision,
        Revision::new(1)
    );
    assert_eq!(outcome(&store, "independent-item"), Some(output.clone()));
    assert!(outcome(&store, "g6-competing-0").is_some());
    assert_eq!(
        success(execute_direct_v1(&resolver(), input, &NoEffects, &NoEffects).unwrap()),
        output
    );
    assert_eq!(sequence(&store), before_sequence + 2);
}

#[test]
fn same_item_race_reports_the_current_revision_without_effects() {
    let (temporary, selector, store) = prepare_fixture();
    let before = graph(&store);
    let before_sequence = sequence(&store);
    let competitor = Arc::new(Mutex::new(Competitor::with_trials(temporary.path(), 1)));
    let instrumented = resolver().with_store_boundary_observer_v1({
        let competitor = Arc::clone(&competitor);
        move |boundary| {
            if boundary == DirectTransactionBoundaryV1::BeforeBegin {
                competitor.lock().unwrap().commit_item(0, "i01");
            }
        }
    });
    let input = request(&selector, set(&before, "i01", "parent"), "same-item-stale");
    let DirectResponseV1::Failure(error) =
        execute_direct_v1(&instrumented, input, &NoEffects, &NoEffects).unwrap()
    else {
        panic!("stale item fence committed");
    };
    competitor.lock().unwrap().finish();
    let error = error.into_value();
    assert!(validate_daemon_free_document_v1("podway.error/v2", &error));
    assert_eq!(error["code"], "ITEM_REVISION_CONFLICT");
    assert_eq!(error["details"]["current_revision"], 1);
    assert_eq!(sequence(&store), before_sequence + 1);
    assert!(outcome(&store, "same-item-stale").is_none());
    assert_eq!(
        item_fences(&graph(&store), "i01").expected_item_revision,
        Revision::new(1)
    );
}

#[test]
fn in_transaction_workspace_revalidation_keeps_typed_errors_and_exact_retry() {
    for (route, unsafe_git) in [
        "item",
        "repair",
        "record-many",
        "cursor",
        "begin",
        "procedure",
        "reset",
        "archive",
        "start",
        "delegation",
    ]
    .into_iter()
    .flat_map(|route| [false, true].map(|unsafe_git| (route, unsafe_git)))
    {
        let (temporary, selector, store) = prepare_fixture();
        let command = typed_revalidation_command(route, temporary.path(), &selector, &store);
        let before = graph(&store);
        let before_sequence = sequence(&store);
        let root = temporary.path().to_path_buf();
        let config = root.join(".podway/config.yaml");
        let original_config = std::fs::read(&config).ok();
        let instrumented = resolver().with_store_boundary_observer_v1({
            let root = root.clone();
            let config = config.clone();
            move |boundary| {
                if boundary == DirectTransactionBoundaryV1::BeforeBegin {
                    if unsafe_git {
                        std::fs::rename(root.join(".git"), root.join("owned-git")).unwrap();
                        std::os::unix::fs::symlink("owned-git", root.join(".git")).unwrap();
                    } else {
                        std::fs::write(&config, b"invalid: [\n").unwrap();
                    }
                }
            }
        });
        let input = request(&selector, command, "typed-revalidation");
        let response =
            execute_direct_v1(&instrumented, input.clone(), &NoEffects, &NoEffects).unwrap();
        if unsafe_git {
            std::fs::remove_file(root.join(".git")).unwrap();
            std::fs::rename(root.join("owned-git"), root.join(".git")).unwrap();
        } else if let Some(original_config) = original_config {
            std::fs::write(&config, original_config).unwrap();
        } else {
            std::fs::remove_file(&config).unwrap();
        }
        rejection(
            response,
            if unsafe_git {
                "WORKSPACE_PATH_UNSAFE"
            } else {
                "WORKSPACE_CONFIG_INVALID"
            },
        );
        assert_eq!(graph(&store), before);
        assert_eq!(sequence(&store), before_sequence);
        assert!(outcome(&store, "typed-revalidation").is_none());
        let output = success(
            execute_direct_v1(&resolver(), input.clone(), &Clock, &NativeDirectIdSourceV1).unwrap(),
        );
        assert_eq!(
            success(execute_direct_v1(&resolver(), input, &NoEffects, &NoEffects).unwrap()),
            output
        );
        assert_eq!(sequence(&store), before_sequence + 1);
    }
}

fn typed_revalidation_command(
    route: &str,
    root: &std::path::Path,
    selector: &WorktreeSelectorV1,
    store: &DirectStoreV1,
) -> DirectCommandV1 {
    let execute = |command, key| {
        success(
            execute_direct_v1(
                &resolver(),
                request(selector, command, key),
                &Clock,
                &NativeDirectIdSourceV1,
            )
            .unwrap(),
        )
    };
    let identity = |current: &GraphSessionStateV2| SessionIdentityPreconditionsWireV1 {
        expected_session_id: current.trace().session_id().clone(),
        expected_session_revision: current.trace().revision(),
    };
    let force = || SessionDeletionModeV1::Force {
        progress_summary: "Replace the isolated revalidation fixture.".into(),
    };
    if route == "begin" {
        execute(
            DirectCommandV1::Session(DirectSessionCommandV1::Reset(SessionResetV1 {
                mode: force(),
                dry_run: false,
                preconditions: identity(&graph(store)),
            })),
            "typed-fixture-reset",
        );
        execute(plain_start(root), "typed-fixture-start");
    }
    if matches!(route, "procedure" | "archive") {
        execute(
            DirectCommandV1::Cursor(DirectCursorCommandV1::Cancel(SessionCancelV1 {
                reason: "Finish the isolated revalidation fixture.".into(),
                preconditions: cursor_fences(&graph(store)),
            })),
            "typed-fixture-cancel",
        );
    }
    if route == "archive" {
        execute(
            DirectCommandV1::Procedure(ProcedureV2MutationCommandV1::SessionTerminalDisposition(
                SessionTerminalDispositionV1 {
                    disposition: TerminalDispositionInputV1::NotRequired {
                        reason: "No external fixture handoff is required.".into(),
                    },
                    actor: None,
                    preconditions: identity(&graph(store)),
                },
            )),
            "typed-fixture-dispose",
        );
    }
    if route == "delegation" {
        let mut source: Value =
            serde_json::from_slice(&std::fs::read(root.join("race.json")).unwrap()).unwrap();
        source["graph"]["nodes"][0]["delegation"] = json!({"binding_ref":"local-specialist", "role_ref":"reviewer",
            "role_snapshot_digest":digest(), "max_rounds":3, "after_accept":"retain"});
        let source = serde_json::to_vec(&source).unwrap();
        std::fs::write(root.join("typed-delegated.json"), &source).unwrap();
        execute(
            DirectCommandV1::Start(DirectStartCommandV1::Replace(SessionStartReplaceV1 {
                start: SessionStartV1 {
                    source: SessionStartSourceV1::Procedure {
                        procedure: "typed-delegated.json".into(),
                    },
                    expected_procedure_digest: Some(
                        prepare_workspace_procedure_source_v2("typed-delegated.json", &source)
                            .unwrap()
                            .digest()
                            .clone(),
                    ),
                    task_title: "Delegated revalidation fixture".into(),
                    dry_run: false,
                },
                mode: force(),
                preconditions: identity(&graph(store)),
            })),
            "typed-fixture-delegation-start",
        );
        execute(
            DirectCommandV1::Procedure(ProcedureV2MutationCommandV1::SessionBegin(
                SessionBeginV1 {
                    initial_goal: None,
                    preconditions: identity(&graph(store)),
                },
            )),
            "typed-fixture-delegation-begin",
        );
        attach(selector, store, "typed-fixture-delegation-attach");
    }
    let current = graph(store);
    match route {
        "item" => set(&current, "i01", "parent"),
        "repair" => DirectCommandV1::WorkspaceRepair,
        "record-many" => state_command(&current, 1),
        "cursor" => DirectCommandV1::Cursor(DirectCursorCommandV1::Cancel(SessionCancelV1 {
            reason: "Cancel the isolated fixture.".into(),
            preconditions: cursor_fences(&current),
        })),
        "begin" => {
            DirectCommandV1::Procedure(ProcedureV2MutationCommandV1::SessionBegin(SessionBeginV1 {
                initial_goal: None,
                preconditions: identity(&current),
            }))
        }
        "procedure" => {
            DirectCommandV1::Procedure(ProcedureV2MutationCommandV1::SessionTerminalDisposition(
                SessionTerminalDispositionV1 {
                    disposition: TerminalDispositionInputV1::NotRequired {
                        reason: "No external fixture handoff is required.".into(),
                    },
                    actor: None,
                    preconditions: identity(&current),
                },
            ))
        }
        "reset" => DirectCommandV1::Session(DirectSessionCommandV1::Reset(SessionResetV1 {
            mode: force(),
            dry_run: false,
            preconditions: identity(&current),
        })),
        "archive" => DirectCommandV1::Session(DirectSessionCommandV1::Archive(SessionArchiveV1 {
            preconditions: identity(&current),
        })),
        "start" => {
            let DirectCommandV1::Start(DirectStartCommandV1::Start(start)) = plain_start(root)
            else {
                unreachable!()
            };
            DirectCommandV1::Start(DirectStartCommandV1::Replace(SessionStartReplaceV1 {
                start,
                mode: force(),
                preconditions: identity(&current),
            }))
        }
        "delegation" => {
            let package = execute(
                DirectCommandV1::Read {
                    command: DirectReadCommandV1::DelegationPackage,
                    expected_session_id: None,
                },
                "typed-fixture-package",
            );
            let workspace = store
                .read(deadline(), |snapshot| {
                    Ok(snapshot.binding().identity().workspace_uuid().clone())
                })
                .unwrap();
            DirectCommandV1::DelegationAccept(DelegationAcceptV1 {
                input: json!({"schema":"podway.delegation-accept-input/v1", "workspace_uuid":workspace,
                    "session_id":current.trace().session_id(), "session_revision":current.trace().revision(),
                    "attempt_id":current.trace().active_attempt().unwrap().attempt_id(), "idempotency_key":"typed-revalidation",
                    "package_digest":package["result"]["package_digest"],
                    "execution":{"binding_ref":"local-specialist", "adapter_id":"fixture", "adapter_version":"1", "primary_host":"external",
                        "scope_kind":"test", "scope_ref":"scope-1", "supervisor_ref":"supervisor-1", "worker_ref":"worker-1", "task_ref":"task-1",
                        "role_ref":"reviewer", "role_snapshot_digest":digest(), "agent_configuration_digest":digest(), "accepted_task_digest":digest()},
                    "rounds_used":1, "result_digest":digest(), "summary":"Verified delegated fixture.", "worker_disposition":"idle", "accepted":true}),
                preconditions: cursor_fences(&current),
            })
        }
        _ => unreachable!(),
    }
}

#[test]
fn attach_rehash_rejects_a_change_after_actual_preparation_and_retries_once() {
    let (temporary, selector, store) = prepare_fixture();
    let before = graph(&store);
    let before_sequence = sequence(&store);
    let artifact = temporary.path().join("artifact.txt");
    let command = DirectCommandV1::Item(DirectItemCommandV1::Attach(ItemAttachV1 {
        item_id: ItemId::new("artifact").unwrap(),
        source: ItemAttachSourceV1::Path {
            path: "artifact.txt".into(),
            media_type: Some("text/plain".into()),
        },
        preconditions: item_fences(&before, "artifact"),
    }));
    let input = request(&selector, command, "g6-attach-after-prehash");
    let events = Arc::new(AtomicUsize::new(0));
    let instrumented = resolver().with_store_boundary_observer_v1({
        let artifact = artifact.clone();
        let events = Arc::clone(&events);
        move |boundary| {
            if boundary == DirectTransactionBoundaryV1::BeforeBegin {
                assert_eq!(events.fetch_add(1, Ordering::SeqCst), 0);
                std::fs::write(&artifact, b"changed after the real attachment prehash\n").unwrap();
            }
        }
    });
    rejection(
        execute_direct_v1(&instrumented, input.clone(), &NoEffects, &NoEffects).unwrap(),
        "ARTIFACT_CHANGED",
    );
    assert_eq!(events.load(Ordering::SeqCst), 1);
    assert_eq!(graph(&store), before);
    assert_eq!(sequence(&store), before_sequence);
    assert!(outcome(&store, "g6-attach-after-prehash").is_none());
    let output = success(
        execute_direct_v1(&resolver(), input.clone(), &Clock, &NativeDirectIdSourceV1).unwrap(),
    );
    assert_eq!(sequence(&store), before_sequence + 1);
    std::fs::remove_file(artifact).unwrap();
    assert_eq!(
        success(execute_direct_v1(&resolver(), input, &NoEffects, &NoEffects).unwrap()),
        output
    );
    assert_eq!(sequence(&store), before_sequence + 1);
    store.verify_integrity(deadline()).unwrap();
}

#[test]
#[ignore = "run explicitly in release profile; actual Engine G6 500 state +500 artifact qualification"]
fn g6_rejects_500_state_and_500_artifact_preparation_races_without_stale_commits() {
    let (temporary, selector, store) = prepare_fixture();
    let competitor = Arc::new(Mutex::new(Competitor::new(temporary.path())));
    let mut state_rejected = 0;
    for trial in 0..TRIALS {
        let before = graph(&store);
        let before_sequence = sequence(&store);
        let key = format!("g6-state-{trial}");
        let input = request(&selector, state_command(&before, trial), &key);
        let competing_output = Arc::new(Mutex::new(None));
        let events = Arc::new(AtomicUsize::new(0));
        let instrumented = resolver().with_store_boundary_observer_v1({
            let competitor = Arc::clone(&competitor);
            let result = Arc::clone(&competing_output);
            let events = Arc::clone(&events);
            move |boundary| {
                if boundary == DirectTransactionBoundaryV1::BeforeBegin {
                    assert_eq!(events.fetch_add(1, Ordering::SeqCst), 0);
                    *result.lock().unwrap() = Some(competitor.lock().unwrap().commit(trial));
                }
            }
        });
        rejection(
            execute_direct_v1(&instrumented, input.clone(), &NoEffects, &NoEffects).unwrap(),
            if trial % 3 == 0 {
                "ITEM_REVISION_CONFLICT"
            } else {
                "SESSION_REVISION_CONFLICT"
            },
        );
        assert_eq!(events.load(Ordering::SeqCst), 1);
        assert!(outcome(&store, &key).is_none());
        let acknowledged = competing_output.lock().unwrap().take().unwrap();
        assert_eq!(sequence(&store), before_sequence + 1);
        assert_eq!(
            outcome(&store, &format!("g6-competing-{trial}")),
            Some(acknowledged)
        );
        let committed = graph(&store);
        let competing_item = if trial % 3 == 0 { "i01" } else { "i16" };
        let competing_fences = item_fences(&before, competing_item);
        let expected = before
            .mutate_active_item_v2(
                &competing_fences.expected_attempt_id,
                &ItemId::new(competing_item).unwrap(),
                competing_fences.expected_item_revision,
                podway_store::ActiveItemMutationV2::Set {
                    value: format!("competing-{trial}"),
                },
                Clock.now(),
            )
            .unwrap();
        assert_eq!(&committed, expected.state());
        assert_eq!(
            committed.trace().revision().get(),
            before.trace().revision().get() + 1
        );
        assert_eq!(
            committed.trace().active_attempt(),
            before.trace().active_attempt()
        );
        // An unchanged retry is still stale. It must not reserve the rejected key.
        rejection(
            execute_direct_v1(&resolver(), input, &NoEffects, &NoEffects).unwrap(),
            if trial % 3 == 0 {
                "ITEM_REVISION_CONFLICT"
            } else {
                "SESSION_REVISION_CONFLICT"
            },
        );
        assert_eq!(graph(&store), committed);
        assert_eq!(sequence(&store), before_sequence + 1);
        assert!(outcome(&store, &key).is_none());
        if trial % 3 != 2 {
            let fresh = request(&selector, state_command(&committed, trial), &key);
            let output = success(
                execute_direct_v1(&resolver(), fresh.clone(), &Clock, &NativeDirectIdSourceV1)
                    .unwrap(),
            );
            assert_eq!(
                success(execute_direct_v1(&resolver(), fresh, &NoEffects, &NoEffects).unwrap()),
                output
            );
            assert_eq!(sequence(&store), before_sequence + 2);
            assert_eq!(outcome(&store, &key), Some(output));
        }
        state_rejected += 1;
    }
    competitor.lock().unwrap().finish();
    // The final rejected complete key can execute once with fresh fences.
    let fresh = request(&selector, complete(&graph(&store)), "g6-state-497");
    let output = success(
        execute_direct_v1(&resolver(), fresh.clone(), &Clock, &NativeDirectIdSourceV1).unwrap(),
    );
    assert_eq!(
        success(execute_direct_v1(&resolver(), fresh, &NoEffects, &NoEffects).unwrap()),
        output
    );
    assert_eq!(
        graph(&store)
            .trace()
            .active_attempt()
            .unwrap()
            .graph_node_id()
            .as_str(),
        "second"
    );
    attach(&selector, &store, "race-attach-second");

    let artifact = temporary.path().join("artifact.txt");
    let original = std::fs::read(&artifact).unwrap();
    let mut artifact_rejected = 0;
    for trial in 0..TRIALS {
        std::fs::write(&artifact, &original).unwrap();
        let before = graph(&store);
        let before_sequence = sequence(&store);
        let key = format!("g6-artifact-{trial}");
        let events = Arc::new(AtomicUsize::new(0));
        let instrumented = resolver().with_store_boundary_observer_v1({
            let artifact = artifact.clone();
            let events = Arc::clone(&events);
            move |boundary| {
                if boundary == DirectTransactionBoundaryV1::BeforeBegin {
                    assert_eq!(events.fetch_add(1, Ordering::SeqCst), 0);
                    std::fs::write(&artifact, format!("changed after preparation {trial}\n"))
                        .unwrap();
                }
            }
        });
        rejection(
            execute_direct_v1(
                &instrumented,
                request(&selector, complete(&before), &key),
                &NoEffects,
                &NoEffects,
            )
            .unwrap(),
            "ARTIFACT_CHANGED",
        );
        assert_eq!(events.load(Ordering::SeqCst), 1);
        assert_eq!(graph(&store), before);
        assert_eq!(sequence(&store), before_sequence);
        assert!(outcome(&store, &key).is_none());
        artifact_rejected += 1;
    }
    // Restoring the actual attached bytes lets the exact rejected key apply once.
    std::fs::write(&artifact, original).unwrap();
    let fresh = request(&selector, complete(&graph(&store)), "g6-artifact-499");
    let output = success(
        execute_direct_v1(&resolver(), fresh.clone(), &Clock, &NativeDirectIdSourceV1).unwrap(),
    );
    std::fs::remove_file(&artifact).unwrap();
    assert_eq!(
        success(execute_direct_v1(&resolver(), fresh, &NoEffects, &NoEffects).unwrap()),
        output
    );
    assert_eq!(
        graph(&store)
            .trace()
            .active_attempt()
            .unwrap()
            .graph_node_id()
            .as_str(),
        "third"
    );
    store.verify_integrity(deadline()).unwrap();
    assert_eq!((state_rejected, artifact_rejected), (500, 500));
    println!(
        "G6 actual Engine: {state_rejected} state +{artifact_rejected} artifact races rejected; 0 stale commits"
    );
}
