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

pub(super) fn graph(store: &DirectStoreV1) -> GraphSessionStateV2 {
    store
        .prepare_graph(deadline())
        .unwrap()
        .unwrap()
        .graph()
        .clone()
}

pub(super) fn item_fences(
    graph: &GraphSessionStateV2,
    item: &str,
) -> ItemMutationPreconditionsWireV1 {
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

pub(super) fn cursor_fences(graph: &GraphSessionStateV2) -> SessionMutationPreconditionsWireV1 {
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

pub(super) fn prepare_fixture() -> (tempfile::TempDir, WorktreeSelectorV1, DirectStoreV1) {
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
        Self::with_child(root, trials, "int_v2dfc006_races::preparation_race_child")
    }
    fn with_child(root: &std::path::Path, trials: usize, test: &str) -> Self {
        assert!((1..=TRIALS).contains(&trials));
        // Temporary roots can have symlinked parents, including macOS /var.
        // The child passes these bytes to the fail-closed resolver unchanged.
        let root = std::fs::canonicalize(root).expect("canonical owned fixture root");
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", test, "--nocapture", "--test-threads=1"])
            .env("PODWAY_ENGINE_G6_OWNED_FIXTURE", &root)
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

#[test]
fn competing_child_resolves_a_symlinked_temporary_parent() {
    let (temporary, _selector, store) = prepare_fixture();
    let canonical = std::fs::canonicalize(temporary.path()).unwrap();
    let parent = tempfile::tempdir().unwrap();
    let alias = parent.path().join("temporary-parent");
    std::os::unix::fs::symlink(canonical.parent().unwrap(), &alias).unwrap();
    let linked = alias.join(canonical.file_name().unwrap());
    assert_ne!(linked, canonical);
    assert!(
        std::fs::symlink_metadata(&alias)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    let mut competitor = Competitor::with_trials(&linked, 1);
    let committed = competitor.commit_item(0, "i01");
    competitor.finish();
    assert_eq!(outcome(&store, "g6-competing-0"), Some(committed));
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
        let before_binding = store
            .read(deadline(), |snapshot| Ok(snapshot.binding().clone()))
            .unwrap();
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
        let DirectResponseV1::Failure(error) = response else {
            panic!("{route}: transaction revalidation committed");
        };
        let error = error.into_value();
        assert!(validate_daemon_free_document_v1("podway.error/v2", &error));
        assert_eq!(
            error["code"],
            if unsafe_git {
                "WORKSPACE_PATH_UNSAFE"
            } else {
                "WORKSPACE_CONFIG_INVALID"
            },
            "{route}"
        );
        assert_eq!(error["exit_code"], 5, "{route}");
        assert_eq!(error["retryable"], false, "{route}");
        assert_eq!(
            error["details"],
            if unsafe_git {
                json!({})
            } else {
                json!({"guidance":"Remove any non-prod mode from .podway/config.yaml, then run podway reset --all with explicit authorization."})
            },
            "{route}"
        );
        assert_eq!(graph(&store), before);
        assert_eq!(sequence(&store), before_sequence);
        assert_eq!(
            store
                .read(deadline(), |snapshot| Ok(snapshot.binding().clone()))
                .unwrap(),
            before_binding,
            "{route}"
        );
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
fn attach_stamp_check_rejects_a_change_after_actual_preparation_and_retries_once() {
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

fn artifact_route_fixture(route: &str) -> (tempfile::TempDir, WorktreeSelectorV1, DirectStoreV1) {
    let (temporary, selector, store) = prepare_fixture();
    if route == "decide" {
        let source = serde_json::to_vec(&json!({"schema":"podway.procedure/v2", "id":"artifact-decision", "version":"1",
            "name":"Artifact decision", "purpose":"Verify decision artifact preparation.",
            "node_definitions":{"choice":{"type":"decision", "title":"Choose", "objective":"Use a current artifact.",
                "prompt":"Continue?", "items":[{"id":"artifact", "type":"artifact", "prompt":"Attach report.", "required":true}],
                "options":[{"id":"continue", "label":"Continue"}], "reason":{"required":true}},
                "finish":{"type":"action", "title":"Finish", "intent":"Finish the fixture."}},
            "graph":{"entry":"choice", "nodes":[{"id":"choice", "use":"choice", "routes":{"continue":{"to":"finish", "effect":"advance"}}},
                {"id":"finish", "use":"finish", "terminal":true}]}})).unwrap();
        std::fs::write(temporary.path().join("decision.json"), &source).unwrap();
        let current = graph(&store);
        success(
            execute_direct_v1(
                &resolver(),
                request(
                    &selector,
                    DirectCommandV1::Start(DirectStartCommandV1::Replace(SessionStartReplaceV1 {
                        start: SessionStartV1 {
                            source: SessionStartSourceV1::Procedure {
                                procedure: "decision.json".into(),
                            },
                            expected_procedure_digest: Some(
                                prepare_workspace_procedure_source_v2("decision.json", &source)
                                    .unwrap()
                                    .digest()
                                    .clone(),
                            ),
                            task_title: "Artifact decision fixture".into(),
                            dry_run: false,
                        },
                        mode: SessionDeletionModeV1::Force {
                            progress_summary: "Replace the isolated fixture.".into(),
                        },
                        preconditions: SessionIdentityPreconditionsWireV1 {
                            expected_session_id: current.trace().session_id().clone(),
                            expected_session_revision: current.trace().revision(),
                        },
                    })),
                    "task014-decision-start",
                ),
                &Clock,
                &NativeDirectIdSourceV1,
            )
            .unwrap(),
        );
        let current = graph(&store);
        success(
            execute_direct_v1(
                &resolver(),
                request(
                    &selector,
                    DirectCommandV1::Procedure(ProcedureV2MutationCommandV1::SessionBegin(
                        SessionBeginV1 {
                            initial_goal: None,
                            preconditions: SessionIdentityPreconditionsWireV1 {
                                expected_session_id: current.trace().session_id().clone(),
                                expected_session_revision: current.trace().revision(),
                            },
                        },
                    )),
                    "task014-decision-begin",
                ),
                &Clock,
                &NativeDirectIdSourceV1,
            )
            .unwrap(),
        );
    }
    (temporary, selector, store)
}

fn artifact_command(current: &GraphSessionStateV2, route: &str) -> DirectCommandV1 {
    match route {
        "attach" => DirectCommandV1::Item(DirectItemCommandV1::Attach(ItemAttachV1 {
            item_id: ItemId::new("artifact").unwrap(),
            source: ItemAttachSourceV1::Path {
                path: "artifact.txt".into(),
                media_type: Some("text/plain".into()),
            },
            preconditions: item_fences(current, "artifact"),
        })),
        "record" => DirectCommandV1::RecordMany(ItemRecordManyV1 {
            preconditions: cursor_fences(current),
            operations: vec![ItemRecordManyOperationV1 {
                item_id: ItemId::new("artifact").unwrap(),
                expected_item_revision: item_fences(current, "artifact").expected_item_revision,
                disposition: ItemRecordManyDispositionV1::Record {
                    record: ItemRecordValueV1::Artifact {
                        path: Some("artifact.txt".into()),
                        reference: None,
                        digest: None,
                        size_bytes: None,
                        media_type: Some("text/plain".into()),
                    },
                },
            }],
        }),
        "complete" => complete(current),
        "decide" => DirectCommandV1::Procedure(ProcedureV2MutationCommandV1::SessionDecide(
            podway_protocol::SessionDecideV2 {
                option_id: podway_core::OptionId::new("continue").unwrap(),
                reason: "Use the verified report.".into(),
                actor: None,
                preconditions: cursor_fences(current),
                expected_goal_revision: None,
            },
        )),
        _ => unreachable!(),
    }
}

#[test]
fn artifact_stamps_reject_same_size_rewrites_renames_and_mid_hash_changes_on_every_route() {
    for route in ["attach", "record", "complete", "decide"] {
        for change in ["rewrite", "rename", "mid-hash"] {
            let (temporary, selector, store) = artifact_route_fixture(route);
            let path = temporary.path().join("artifact.txt");
            let original = vec![b'a'; 128 * 1024];
            std::fs::write(&path, &original).unwrap();
            attach(&selector, &store, "task014-baseline-attach");
            let before = graph(&store);
            let before_sequence = sequence(&store);
            let input = request(
                &selector,
                artifact_command(&before, route),
                "task014-stamp-race",
            );
            let entered = Arc::new(AtomicUsize::new(0));
            let streamed = Arc::new(AtomicUsize::new(0));
            let instrumented = resolver()
                .with_store_boundary_observer_v1({
                    let path = path.clone();
                    let entered = Arc::clone(&entered);
                    let original = original.clone();
                    move |boundary| {
                        if boundary == DirectTransactionBoundaryV1::BeforeBegin {
                            assert_eq!(entered.fetch_add(1, Ordering::SeqCst), 0);
                            match change {
                                "rewrite" => {
                                    std::fs::write(&path, vec![b'b'; original.len()]).unwrap()
                                }
                                "rename" => {
                                    let replacement = path.with_extension("replacement");
                                    std::fs::write(&replacement, &original).unwrap();
                                    std::fs::rename(replacement, &path).unwrap();
                                }
                                "mid-hash" => (),
                                _ => unreachable!(),
                            }
                        }
                    }
                })
                .with_artifact_hash_observer_v1({
                    let path = path.clone();
                    let entered = Arc::clone(&entered);
                    let streamed = Arc::clone(&streamed);
                    move |_, bytes| {
                        assert_eq!(
                            entered.load(Ordering::SeqCst),
                            0,
                            "artifact bytes read after writer admission"
                        );
                        streamed.fetch_add(1, Ordering::SeqCst);
                        if change == "mid-hash" && bytes == 64 * 1024 {
                            std::fs::write(&path, vec![b'b'; 128 * 1024]).unwrap();
                        }
                    }
                });
            rejection(
                execute_direct_v1(&instrumented, input.clone(), &NoEffects, &NoEffects).unwrap(),
                "ARTIFACT_CHANGED",
            );
            assert!(streamed.load(Ordering::SeqCst) > 0);
            assert_eq!(entered.load(Ordering::SeqCst), 1);
            assert_eq!(graph(&store), before);
            assert_eq!(sequence(&store), before_sequence);
            assert!(outcome(&store, "task014-stamp-race").is_none());
            std::fs::write(&path, &original).unwrap();
            let output = success(
                execute_direct_v1(&resolver(), input.clone(), &Clock, &NativeDirectIdSourceV1)
                    .unwrap(),
            );
            assert_eq!(sequence(&store), before_sequence + 1);
            std::fs::remove_file(path).unwrap();
            assert_eq!(
                success(execute_direct_v1(&resolver(), input, &NoEffects, &NoEffects).unwrap()),
                output
            );
            assert_eq!(sequence(&store), before_sequence + 1);
            store.verify_integrity(deadline()).unwrap();
        }
    }
}

#[test]
fn every_artifact_stamp_is_checked_in_batches_and_required_item_sets() {
    for route in ["record", "complete", "decide"] {
        let (temporary, selector, store) = prepare_fixture();
        let items = json!([
            {"id":"artifact", "type":"artifact", "prompt":"Attach first report.", "required":true},
            {"id":"artifact2", "type":"artifact", "prompt":"Attach second report.", "required":true}
        ]);
        let work = if route == "decide" {
            json!({"type":"decision", "title":"Choose", "objective":"Use current reports.",
                "prompt":"Continue?", "items":items,
                "options":[{"id":"continue", "label":"Continue"}], "reason":{"required":true}})
        } else {
            json!({"type":"action", "title":"Work", "intent":"Use current reports.", "items":items})
        };
        let first = if route == "decide" {
            json!({"id":"work", "use":"work", "routes":{"continue":{"to":"finish", "effect":"advance"}}})
        } else {
            json!({"id":"work", "use":"work", "next":"finish"})
        };
        let source = serde_json::to_vec(&json!({"schema":"podway.procedure/v2", "id":"two-artifacts", "version":"1",
            "name":"Two artifacts", "purpose":"Check every prepared artifact stamp.",
            "node_definitions":{"work":work, "finish":{"type":"action", "title":"Finish", "intent":"Finish the fixture."}},
            "graph":{"entry":"work", "nodes":[first, {"id":"finish", "use":"finish", "terminal":true}]}})).unwrap();
        std::fs::write(temporary.path().join("two-artifacts.json"), &source).unwrap();
        let current = graph(&store);
        success(
            execute_direct_v1(
                &resolver(),
                request(
                    &selector,
                    DirectCommandV1::Start(DirectStartCommandV1::Replace(SessionStartReplaceV1 {
                        start: SessionStartV1 {
                            source: SessionStartSourceV1::Procedure {
                                procedure: "two-artifacts.json".into(),
                            },
                            expected_procedure_digest: Some(
                                prepare_workspace_procedure_source_v2(
                                    "two-artifacts.json",
                                    &source,
                                )
                                .unwrap()
                                .digest()
                                .clone(),
                            ),
                            task_title: "Two-artifact fixture".into(),
                            dry_run: false,
                        },
                        mode: SessionDeletionModeV1::Force {
                            progress_summary: "Replace the isolated fixture.".into(),
                        },
                        preconditions: SessionIdentityPreconditionsWireV1 {
                            expected_session_id: current.trace().session_id().clone(),
                            expected_session_revision: current.trace().revision(),
                        },
                    })),
                    "task014-two-artifacts-start",
                ),
                &Clock,
                &NativeDirectIdSourceV1,
            )
            .unwrap(),
        );
        let current = graph(&store);
        success(
            execute_direct_v1(
                &resolver(),
                request(
                    &selector,
                    DirectCommandV1::Procedure(ProcedureV2MutationCommandV1::SessionBegin(
                        SessionBeginV1 {
                            initial_goal: None,
                            preconditions: SessionIdentityPreconditionsWireV1 {
                                expected_session_id: current.trace().session_id().clone(),
                                expected_session_revision: current.trace().revision(),
                            },
                        },
                    )),
                    "task014-two-artifacts-begin",
                ),
                &Clock,
                &NativeDirectIdSourceV1,
            )
            .unwrap(),
        );
        let original = vec![b'a'; 128 * 1024];
        for item in ["artifact", "artifact2"] {
            std::fs::write(temporary.path().join(format!("{item}.txt")), &original).unwrap();
            success(
                execute_direct_v1(
                    &resolver(),
                    request(
                        &selector,
                        DirectCommandV1::Item(DirectItemCommandV1::Attach(ItemAttachV1 {
                            item_id: ItemId::new(item).unwrap(),
                            source: ItemAttachSourceV1::Path {
                                path: format!("{item}.txt"),
                                media_type: Some("text/plain".into()),
                            },
                            preconditions: item_fences(&graph(&store), item),
                        })),
                        &format!("task014-two-artifacts-{item}"),
                    ),
                    &Clock,
                    &NativeDirectIdSourceV1,
                )
                .unwrap(),
            );
        }
        let before = graph(&store);
        let before_sequence = sequence(&store);
        let command = if route == "record" {
            DirectCommandV1::RecordMany(ItemRecordManyV1 {
                preconditions: cursor_fences(&before),
                operations: ["artifact", "artifact2"]
                    .into_iter()
                    .map(|item| ItemRecordManyOperationV1 {
                        item_id: ItemId::new(item).unwrap(),
                        expected_item_revision: item_fences(&before, item).expected_item_revision,
                        disposition: ItemRecordManyDispositionV1::Record {
                            record: ItemRecordValueV1::Artifact {
                                path: Some(format!("{item}.txt")),
                                reference: None,
                                digest: None,
                                size_bytes: None,
                                media_type: Some("text/plain".into()),
                            },
                        },
                    })
                    .collect(),
            })
        } else {
            artifact_command(&before, route)
        };
        let input = request(&selector, command, "task014-two-artifacts-race");
        let entered = Arc::new(AtomicUsize::new(0));
        let prepared = Arc::new(AtomicUsize::new(0));
        let second_path = temporary.path().join("artifact2.txt");
        let instrumented = resolver()
            .with_store_boundary_observer_v1({
                let entered = Arc::clone(&entered);
                let prepared = Arc::clone(&prepared);
                let second_path = second_path.clone();
                move |boundary| {
                    if boundary == DirectTransactionBoundaryV1::BeforeBegin {
                        assert_eq!(entered.fetch_add(1, Ordering::SeqCst), 0);
                        assert_eq!(prepared.load(Ordering::SeqCst), 2);
                        std::fs::write(&second_path, vec![b'b'; 128 * 1024]).unwrap();
                    }
                }
            })
            .with_artifact_hash_observer_v1({
                let entered = Arc::clone(&entered);
                let prepared = Arc::clone(&prepared);
                move |_, bytes| {
                    assert_eq!(
                        entered.load(Ordering::SeqCst),
                        0,
                        "artifact bytes read after writer admission"
                    );
                    if bytes == 128 * 1024 {
                        prepared.fetch_add(1, Ordering::SeqCst);
                    }
                }
            });
        rejection(
            execute_direct_v1(&instrumented, input.clone(), &NoEffects, &NoEffects).unwrap(),
            "ARTIFACT_CHANGED",
        );
        assert_eq!(entered.load(Ordering::SeqCst), 1);
        assert_eq!(graph(&store), before);
        assert_eq!(sequence(&store), before_sequence);
        assert!(outcome(&store, "task014-two-artifacts-race").is_none());
        std::fs::write(&second_path, &original).unwrap();
        let output = success(
            execute_direct_v1(&resolver(), input.clone(), &Clock, &NativeDirectIdSourceV1).unwrap(),
        );
        assert_eq!(sequence(&store), before_sequence + 1);
        for item in ["artifact", "artifact2"] {
            std::fs::remove_file(temporary.path().join(format!("{item}.txt"))).unwrap();
        }
        assert_eq!(
            success(execute_direct_v1(&resolver(), input, &NoEffects, &NoEffects).unwrap()),
            output
        );
        assert_eq!(sequence(&store), before_sequence + 1);
        store.verify_integrity(deadline()).unwrap();
    }
}

#[test]
fn caller_fences_precede_artifact_preparation_failures_and_stamp_changes_on_every_route() {
    for route in ["attach", "record", "complete", "decide"] {
        for change in ["missing-preparation", "changed-stamp"] {
            let (temporary, selector, store) = artifact_route_fixture(route);
            let path = temporary.path().join("artifact.txt");
            let original = vec![b'a'; 128 * 1024];
            std::fs::write(&path, &original).unwrap();
            attach(&selector, &store, "task014-fence-baseline");
            let prior = graph(&store);
            let input = request(
                &selector,
                artifact_command(&prior, route),
                "task014-fence-race",
            );
            // Identical attachments preserve the item revision. Change recorded bytes
            // through a real public mutation to stale both item and cursor fences.
            let original = vec![b'b'; 128 * 1024];
            std::fs::write(&path, &original).unwrap();
            attach(&selector, &store, "task014-fence-competing");
            let current = graph(&store);
            let current_sequence = sequence(&store);
            if change == "missing-preparation" {
                std::fs::remove_file(&path).unwrap();
            }
            let instrumented = resolver().with_store_boundary_observer_v1({
                let path = path.clone();
                move |boundary| {
                    if boundary == DirectTransactionBoundaryV1::BeforeBegin
                        && change == "changed-stamp"
                    {
                        std::fs::write(&path, vec![b'c'; 128 * 1024]).unwrap();
                    }
                }
            });
            rejection(
                execute_direct_v1(&instrumented, input, &NoEffects, &NoEffects).unwrap(),
                if route == "attach" {
                    "ITEM_REVISION_CONFLICT"
                } else {
                    "SESSION_REVISION_CONFLICT"
                },
            );
            assert_eq!(graph(&store), current);
            assert_eq!(sequence(&store), current_sequence);
            assert!(outcome(&store, "task014-fence-race").is_none());
            std::fs::write(&path, &original).unwrap();
            let corrected = request(
                &selector,
                artifact_command(&current, route),
                "task014-fence-race",
            );
            let output = success(
                execute_direct_v1(
                    &resolver(),
                    corrected.clone(),
                    &Clock,
                    &NativeDirectIdSourceV1,
                )
                .unwrap(),
            );
            assert_eq!(sequence(&store), current_sequence + 1);
            std::fs::remove_file(path).unwrap();
            assert_eq!(
                success(execute_direct_v1(&resolver(), corrected, &NoEffects, &NoEffects).unwrap()),
                output
            );
            assert_eq!(sequence(&store), current_sequence + 1);
            store.verify_integrity(deadline()).unwrap();
        }
    }
}

#[test]
fn typed_citation_bounds_reject_before_effects_and_preserve_fences_retry_and_replay() {
    use super::int_v2dfc006_runtime_ports::Engine;
    use podway_core::{CriterionId, GraphNodeId};
    use podway_protocol::{GoalAssessCriterionV2, GoalCriterionWireV2, GoalDefineV2};

    let items: Vec<Value> = (1..=4)
        .map(|index| {
            json!({"id":format!("note{index}"),
            "type":"text", "prompt":"Record a local citation.", "required":false, "max_length":1000})
        })
        .collect();
    let source = json!({"node_definitions":{"assess":{"type":"decision", "title":"Assess", "objective":"Assess the goal.",
            "prompt":"Which outcome?", "items":items, "reason":{"required":true},
            "options":[{"id":"achieved", "label":"Achieved"}, {"id":"failed", "label":"Failed"}, {"id":"superseded", "label":"Superseded"}],
            "assessment":{"target":"session_goal", "outcomes":{"achieved":"achieved", "failed":"not_achieved", "superseded":"superseded"}}},
            "finish":{"type":"action", "title":"Finish", "intent":"Finish the fixture."}},
        "graph":{"entry":"assess", "nodes":[{"id":"assess", "use":"assess", "routes":{
            "achieved":{"to":"finish", "effect":"advance"}, "failed":{"to":"finish", "effect":"advance"}, "superseded":{"to":"finish", "effect":"advance"}}},
            {"id":"finish", "use":"finish", "terminal":true}]}});
    // The shared harness admits source.yaml, which requires a block YAML document.
    let source = format!(
        "schema: podway.procedure/v2\nid: citation-bound\nversion: \"1\"\nname: Citation bound\npurpose: Verify typed citation rejection.\ngoal_tracking: true\nnode_definitions: {}\ngraph: {}\n",
        source["node_definitions"], source["graph"]
    );
    let engine = Engine::new(&source);
    let current = engine.graph();
    engine.success(
        DirectCommandV1::Procedure(ProcedureV2MutationCommandV1::GoalDefine(GoalDefineV2 {
            goal: "Verify the typed citation bound.".into(),
            criteria: vec![GoalCriterionWireV2 {
                criterion_id: CriterionId::new("done").unwrap(),
                statement: "The bound holds.".into(),
            }],
            actor: None,
            preconditions: SessionIdentityPreconditionsWireV1 {
                expected_session_id: current.trace().session_id().clone(),
                expected_session_revision: current.trace().revision(),
            },
        })),
        "citation-define",
    );
    for index in 1..=4 {
        engine.set(
            &format!("note{index}"),
            "Recorded citation.",
            &format!("citation-note{index}"),
        );
    }
    let before = engine.graph();
    let before_sequence = engine.sequence();
    let mut corrected = None;
    for evidence_count in [0, 2, 5] {
        for status in ["satisfied", "invalid"] {
            for stale in [false, true] {
                let mut input = GoalAssessCriterionV2 {
                    criterion_id: CriterionId::new("done").unwrap(),
                    status: status.into(),
                    reason: "Verify the bound.".into(),
                    evidence: (0..evidence_count)
                        .map(|i| GraphNodeId::new(format!("evidence{i}")).unwrap())
                        .collect(),
                    items: (0..5 - evidence_count)
                        .map(|i| ItemId::new(format!("item{i}")).unwrap())
                        .collect(),
                    actor: None,
                    preconditions: cursor_fences(&before),
                    expected_goal_revision: 1,
                };
                if stale {
                    input.preconditions.expected_session_revision = Revision::ZERO;
                }
                let key = format!("citation-bound-{evidence_count}-{status}-{stale}");
                let input_request = request(
                    &engine.selector,
                    DirectCommandV1::Procedure(ProcedureV2MutationCommandV1::GoalAssessCriterion(
                        input.clone(),
                    )),
                    &key,
                );
                rejection(
                    execute_direct_v1(&engine.resolver, input_request, &NoEffects, &NoEffects)
                        .unwrap(),
                    if stale {
                        "SESSION_REVISION_CONFLICT"
                    } else {
                        "REQUEST_INVALID"
                    },
                );
                assert_eq!(engine.graph(), before);
                assert_eq!(engine.sequence(), before_sequence);
                assert!(outcome(&engine.store, &key).is_none());
                if evidence_count == 0 && status == "satisfied" && !stale {
                    input.items = (1..=4)
                        .map(|i| ItemId::new(format!("note{i}")).unwrap())
                        .collect();
                    corrected = Some(request(
                        &engine.selector,
                        DirectCommandV1::Procedure(
                            ProcedureV2MutationCommandV1::GoalAssessCriterion(input),
                        ),
                        &key,
                    ));
                }
            }
        }
    }
    let corrected = corrected.unwrap();
    let output = success(
        execute_direct_v1(
            &engine.resolver,
            corrected.clone(),
            &Clock,
            &NativeDirectIdSourceV1,
        )
        .unwrap(),
    );
    assert_eq!(
        output["result"]["result"]["citations"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    assert_eq!(engine.sequence(), before_sequence + 1);
    assert_eq!(
        success(execute_direct_v1(&engine.resolver, corrected, &NoEffects, &NoEffects).unwrap()),
        output
    );
    assert_eq!(engine.sequence(), before_sequence + 1);
    engine.store.verify_integrity(deadline()).unwrap();
}

#[test]
fn another_process_commits_during_actual_hashing_before_writer_admission() {
    let (temporary, selector, store) = prepare_fixture();
    std::fs::write(
        temporary.path().join("artifact.txt"),
        vec![b'a'; 128 * 1024],
    )
    .unwrap();
    let before = graph(&store);
    let before_sequence = sequence(&store);
    let competitor = Arc::new(Mutex::new(Competitor::with_trials(temporary.path(), 1)));
    let acknowledged = Arc::new(Mutex::new(None));
    let entered = Arc::new(AtomicUsize::new(0));
    let instrumented = resolver()
        .with_store_boundary_observer_v1({
            let entered = Arc::clone(&entered);
            move |boundary| {
                if boundary == DirectTransactionBoundaryV1::BeforeBegin {
                    entered.fetch_add(1, Ordering::SeqCst);
                }
            }
        })
        .with_artifact_hash_observer_v1({
            let entered = Arc::clone(&entered);
            let competitor = Arc::clone(&competitor);
            let acknowledged = Arc::clone(&acknowledged);
            move |_, bytes| {
                assert_eq!(
                    entered.load(Ordering::SeqCst),
                    0,
                    "writer admitted before hashing finished"
                );
                if bytes == 64 * 1024 {
                    *acknowledged.lock().unwrap() =
                        Some(competitor.lock().unwrap().commit_item(0, "i01"));
                }
            }
        });
    let input = request(
        &selector,
        artifact_command(&before, "attach"),
        "task014-hashing-writer",
    );
    success(execute_direct_v1(&instrumented, input, &Clock, &NativeDirectIdSourceV1).unwrap());
    assert_eq!(
        outcome(&store, "g6-competing-0"),
        acknowledged.lock().unwrap().take()
    );
    assert!(outcome(&store, "g6-competing-0").is_some());
    assert_eq!(entered.load(Ordering::SeqCst), 1);
    assert_eq!(sequence(&store), before_sequence + 2);
    competitor.lock().unwrap().finish();
    store.verify_integrity(deadline()).unwrap();
}

#[test]
fn multi_gib_sparse_artifact_attach_and_complete_succeed_at_the_default_timeout() {
    use std::os::unix::fs::MetadataExt;
    const CHILD_ROOT: &str = "PODWAY_ENGINE_LARGE_ARTIFACT_CHILD_ROOT";
    if let Some(root) = std::env::var_os(CHILD_ROOT) {
        assert_eq!(std::env::var_os("TMPDIR"), Some(root.clone()));
        assert!(std::path::Path::new(&root).is_dir());
    } else {
        // This deliberately exceeds the original wait budget. A separate process
        // keeps other worktrees' test-only connections from racing its idle admission.
        let owned = tempfile::Builder::new().prefix("p14-").tempdir().unwrap();
        let root = std::fs::canonicalize(owned.path()).unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "int_v2dfc006_races::multi_gib_sparse_artifact_attach_and_complete_succeed_at_the_default_timeout",
                "--nocapture", "--test-threads=1"])
            .env(CHILD_ROOT, &root).env("TMPDIR", &root)
            .stdin(Stdio::null()).stdout(Stdio::inherit()).stderr(Stdio::inherit())
            .spawn().unwrap();
        let limit = Instant::now() + Duration::from_secs(1200);
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() < limit => std::thread::sleep(Duration::from_millis(10)),
                state => {
                    let _ = child.kill();
                    let _ = child.wait();
                    panic!("owned large-artifact child failed to finish: {state:?}");
                }
            }
        };
        assert!(status.success(), "native large-artifact child failed");
        // TempDir removes only this parent's exact fixture root after the child is reaped.
        return;
    }
    let (temporary, selector, store) = prepare_fixture();
    let path = temporary.path().join("artifact.txt");
    let size = 3 * 1024 * 1024 * 1024_u64;
    let file = std::fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(&path)
        .unwrap();
    file.set_len(size).unwrap();
    drop(file);
    assert!(std::fs::metadata(&path).unwrap().blocks() * 512 < size / 16);
    for route in ["attach", "complete"] {
        let current = graph(&store);
        let mut input = request(
            &selector,
            artifact_command(&current, route),
            &format!("task014-large-{route}"),
        );
        input.deadline = Instant::now() + Duration::from_secs(5);
        let started = Instant::now();
        let output = success(
            execute_direct_v1(&resolver(), input, &Clock, &NativeDirectIdSourceV1).unwrap(),
        );
        assert_eq!(
            output["command"],
            if route == "attach" {
                "item.attach"
            } else {
                "session.complete"
            }
        );
        eprintln!(
            "Task014 native {route}: {size} sparse bytes, {} ms, original 5000 ms wait budget",
            started.elapsed().as_millis()
        );
    }
    store.verify_integrity(deadline()).unwrap();
}

// READS-004: the typed fence-preflight facts operation returns owned facts
// only after the Store read transaction, the process-local connection gate and
// the actual shared/cold-exclusive DirectOpenLease have all been released; a
// real competing writer then commits inside that gap and the original-fence
// mutation fails closed exactly as before.

pub(super) const READS004_RESPONSE: &str = "00000000-0000-4000-8000-000000000077";

pub(super) fn reads004_facts_request(
    selector: &WorktreeSelectorV1,
) -> podway_engine::direct_execution::DirectPreflightFactsRequestV1 {
    podway_engine::direct_execution::DirectPreflightFactsRequestV1 {
        request_id: RequestIdV1::new(READS004_RESPONSE).unwrap(),
        selector: selector.clone(),
        expected_workspace_id: None,
        expected_session_id: None,
        deadline: deadline(),
        observed_at: Rfc3339MillisV1::new("2023-11-14T22:13:20.001Z").unwrap(),
        timeout_ms: 5000,
    }
}

pub(super) fn reads004_facts(
    selector: &WorktreeSelectorV1,
) -> podway_engine::direct_execution::DirectPreflightFactsV1 {
    use podway_engine::direct_api::execute_direct_preflight_facts_v1;
    use podway_engine::direct_execution::DirectPreflightFactsResponseV1;
    match execute_direct_preflight_facts_v1(&resolver(), reads004_facts_request(selector), &Clock)
        .unwrap()
    {
        DirectPreflightFactsResponseV1::Facts(facts) => facts,
        DirectPreflightFactsResponseV1::Failure(error) => {
            panic!("preflight facts failed: {:?}", error.into_value())
        }
    }
}

/// Failure-safe caller reconciliation: the spawned caller is joined on every
/// path, including parent assertion panics, so the fixture is never cleaned up
/// around a live caller. Joins are bounded by the caller's own existing
/// rendezvous and invocation deadlines; no new budget is introduced.
struct Reads004Caller<T> {
    handle: Option<std::thread::JoinHandle<T>>,
}
impl<T: Send + 'static> Reads004Caller<T> {
    fn spawn(body: impl FnOnce() -> T + Send + 'static) -> Self {
        Self {
            handle: Some(std::thread::spawn(body)),
        }
    }
    fn join(mut self) -> std::thread::Result<T> {
        self.handle
            .take()
            .expect("reads004 caller already joined")
            .join()
    }
}
impl<T> Drop for Reads004Caller<T> {
    fn drop(&mut self) {
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

/// The post-competitor state baseline both races compare after the rejected
/// original-fence mutation: committed sequence, session revision and the
/// complete active-attempt slot map (identifier, revision, value presence).
fn reads004_state_baseline(store: &DirectStoreV1) -> (u64, u64, Vec<(String, u64, bool)>) {
    let state = graph(store);
    let attempt = state.trace().active_attempt().unwrap().attempt_id();
    let memory = state
        .workflow_memory()
        .attempts()
        .iter()
        .find(|memory| memory.attempt_id() == attempt)
        .unwrap();
    let slots = memory
        .item_slots()
        .iter()
        .map(|slot| {
            (
                slot.item_id().as_str().to_owned(),
                slot.revision().get(),
                slot.value().is_some(),
            )
        })
        .collect();
    (sequence(store), state.trace().revision().get(), slots)
}

#[test]
fn reads004_cold_facts_release_all_guards_before_a_competing_commit() {
    use podway_core::SessionLifecycle;

    let (temporary, selector, store) = prepare_fixture();
    // Controlled cold precondition: force a full truncate checkpoint so every
    // committed frame reaches the main database, then remove both sidecars —
    // the lossless cold Store state the classification tests build. Podway
    // never creates sidecars during an unmarked cold read, so no intervening
    // actor exists between this setup and the facts read.
    {
        let connection = rusqlite::Connection::open(store.layout().database()).unwrap();
        connection
            .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
            .unwrap();
    }
    let wal = store.layout().wal();
    let shm = store.layout().shm();
    if wal.exists() {
        assert_eq!(
            std::fs::metadata(&wal).unwrap().len(),
            0,
            "fixture WAL still holds uncheckpointed frames"
        );
        std::fs::remove_file(&wal).unwrap();
    }
    if shm.exists() {
        std::fs::remove_file(&shm).unwrap();
    }
    assert!(!wal.exists() && !shm.exists());
    let before = sequence(&store);
    let facts_selector = selector.clone();
    let mutation_selector = selector.clone();
    // Caller 1 runs the real facts facade on its own thread and pauses at a
    // lease-free rendezvous only after the facade returned successfully, with
    // the read transaction, connection gate and lease already dropped by the
    // Store read boundary's RAII scope. It resumes only on the explicit
    // channel below and returns its final response for exact assertion; the
    // guard joins it on every path before the fixture is dropped.
    let (arrive, arrive_rx) = mpsc::channel::<(SessionId, Revision, AttemptId, Revision)>();
    let (resume, resume_rx) = mpsc::channel::<()>();
    let caller = Reads004Caller::spawn(move || {
        let facts = reads004_facts(&facts_selector);
        assert_eq!(facts.lifecycle, SessionLifecycle::Running);
        let item_revision = facts
            .item_revisions
            .iter()
            .find(|(id, _)| id == "i01")
            .expect("active item fence")
            .1;
        arrive
            .send((
                facts.session_id.clone(),
                facts.session_revision,
                facts.attempt_id.clone().expect("running attempt fence"),
                item_revision,
            ))
            .expect("test parent left the arrival rendezvous");
        resume_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("caller 1 missed the bounded resume rendezvous");
        let command = DirectCommandV1::Item(DirectItemCommandV1::Set(ItemSetV1 {
            item_id: ItemId::new("i01").unwrap(),
            value: "reads004-cold-caller".into(),
            preconditions: ItemMutationPreconditionsWireV1 {
                expected_session_id: facts.session_id.clone(),
                expected_attempt_id: facts.attempt_id.clone().unwrap(),
                expected_item_revision: item_revision,
            },
        }));
        execute_direct_v1(
            &resolver(),
            request(&mutation_selector, command, "reads004-cold-caller"),
            &Clock,
            &NativeDirectIdSourceV1,
        )
    });
    let (_session_id, session_revision, _attempt_id, item_revision) = arrive_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("facts caller missed the bounded arrival rendezvous");
    // The real competing writer commits on this thread while caller 1 is
    // paused at the post-facts rendezvous: its own lease, gate and write
    // admission can succeed only because the facts read released every guard
    // it briefly held.
    success(
        execute_direct_v1(
            &resolver(),
            request(
                &selector,
                set(&graph(&store), "i01", "reads004-cold-competing"),
                "reads004-cold-competing",
            ),
            &Clock,
            &NativeDirectIdSourceV1,
        )
        .unwrap(),
    );
    // Confirm the competing commit, and capture the post-competitor state
    // baseline, before caller 1 resumes.
    assert!(outcome(&store, "reads004-cold-competing").is_some());
    assert_eq!(sequence(&store), before + 1);
    let baseline = reads004_state_baseline(&store);
    resume
        .send(())
        .expect("caller 1 already left the rendezvous");
    let response = caller.join().expect("caller 1 thread panicked").unwrap();
    let DirectResponseV1::Failure(error) = response else {
        panic!("stale facts committed")
    };
    let value = error.into_value();
    assert_eq!(value["code"], "ITEM_REVISION_CONFLICT");
    assert_eq!(value["details"]["expected_revision"], item_revision.get());
    let final_graph = graph(&store);
    assert_eq!(
        value["details"]["current_revision"],
        item_fences(&final_graph, "i01")
            .expected_item_revision
            .get()
    );
    // Full state equality with the post-competitor baseline: the rejected
    // mutation left no outcome, revision, slot or sequence effect, and the
    // Store stays coherent.
    assert_eq!(reads004_state_baseline(&store), baseline);
    assert_eq!(outcome(&store, "reads004-cold-caller"), None);
    assert!(outcome(&store, "reads004-cold-competing").is_some());
    assert_eq!(
        final_graph.trace().revision().get(),
        session_revision.get() + 1
    );
    store.verify_integrity(deadline()).unwrap();
    drop(temporary);
}

#[test]
fn reads004_shared_facts_release_all_guards_with_warm_sidecars() {
    use podway_core::SessionLifecycle;

    let (temporary, selector, store) = prepare_fixture();
    // Warm precondition straight from the fixture: its clean Store closes
    // leave both SQLite sidecars in place, so every admission here takes the
    // shared lease branch (maintenance.rs returns Shared when both sidecars
    // exist). No intervening actor touches the fixture before the facts read.
    assert!(
        store.layout().wal().exists() && store.layout().shm().exists(),
        "fixture is not warm: a SQLite sidecar is missing"
    );
    let before = sequence(&store);
    assert!(
        store.layout().wal().exists() && store.layout().shm().exists(),
        "fixture lost its warm sidecars before the facts read"
    );
    let facts_selector = selector.clone();
    let mutation_selector = selector.clone();
    // Caller 1 pauses at the same lease-free post-return rendezvous; the
    // shared lease and connection gate it briefly held must already be free.
    // The guard joins the caller on every path before the fixture is dropped.
    let (arrive, arrive_rx) = mpsc::channel::<(SessionId, Revision, AttemptId)>();
    let (resume, resume_rx) = mpsc::channel::<()>();
    let caller = Reads004Caller::spawn(move || {
        let facts = reads004_facts(&facts_selector);
        assert_eq!(facts.lifecycle, SessionLifecycle::Running);
        arrive
            .send((
                facts.session_id.clone(),
                facts.session_revision,
                facts.attempt_id.clone().expect("running attempt fence"),
            ))
            .expect("test parent left the arrival rendezvous");
        resume_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("caller 1 missed the bounded resume rendezvous");
        let command = DirectCommandV1::Cursor(DirectCursorCommandV1::Complete(SessionCompleteV1 {
            preconditions: SessionMutationPreconditionsWireV1 {
                expected_session_id: facts.session_id.clone(),
                expected_session_revision: facts.session_revision,
                expected_attempt_id: facts.attempt_id.clone().unwrap(),
            },
        }));
        execute_direct_v1(
            &resolver(),
            request(&mutation_selector, command, "reads004-shared-caller"),
            &Clock,
            &NativeDirectIdSourceV1,
        )
    });
    let (_session_id, session_revision, _attempt_id) = arrive_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("facts caller missed the bounded arrival rendezvous");
    // The real competing writer commits on this thread while caller 1 is
    // paused, through the same process-local connection gate the facts read
    // must have released.
    success(
        execute_direct_v1(
            &resolver(),
            request(
                &selector,
                set(&graph(&store), "i01", "reads004-shared-competing"),
                "reads004-shared-competing",
            ),
            &Clock,
            &NativeDirectIdSourceV1,
        )
        .unwrap(),
    );
    // Confirm the competing commit, and capture the post-competitor state
    // baseline, before caller 1 resumes.
    assert!(outcome(&store, "reads004-shared-competing").is_some());
    assert_eq!(sequence(&store), before + 1);
    let baseline = reads004_state_baseline(&store);
    resume
        .send(())
        .expect("caller 1 already left the rendezvous");
    let response = caller.join().expect("caller 1 thread panicked").unwrap();
    let DirectResponseV1::Failure(error) = response else {
        panic!("stale facts committed")
    };
    let value = error.into_value();
    assert_eq!(value["code"], "SESSION_REVISION_CONFLICT");
    assert_eq!(
        value["details"]["expected_revision"],
        session_revision.get()
    );
    let final_graph = graph(&store);
    assert_eq!(
        value["details"]["current_revision"],
        final_graph.trace().revision().get()
    );
    // Full state equality with the post-competitor baseline: the session is
    // still running, the rejected key has no outcome, no slot or sequence
    // effect remains, and the Store stays coherent on the warm sidecars.
    assert_eq!(final_graph.trace().lifecycle(), SessionLifecycle::Running);
    assert_eq!(reads004_state_baseline(&store), baseline);
    assert_eq!(outcome(&store, "reads004-shared-caller"), None);
    assert!(outcome(&store, "reads004-shared-competing").is_some());
    assert_eq!(
        final_graph.trace().revision().get(),
        session_revision.get() + 1
    );
    store.verify_integrity(deadline()).unwrap();
    drop(temporary);
}

// Uses the existing owned pipe/child guard. Readiness is sent only after all
// startup reads have returned, so the parent can then make its Store cold.
#[test]
fn admit002_competing_writer_child() {
    let Some(root) = std::env::var_os("PODWAY_ENGINE_G6_OWNED_FIXTURE") else {
        return;
    };
    let root = std::path::PathBuf::from(root);
    let path = LosslessPathV1::from_raw_bytes(
        root.as_os_str().as_bytes(),
        DiagnosticPathDisplayV1::new(root.to_string_lossy()).unwrap(),
    )
    .unwrap();
    let selector = WorktreeSelectorV1::new(WORKTREE_SELECTOR_VERSION_V1, None, path).unwrap();
    let (_, store) = resolver()
        .resolve_existing(selector.clone(), None, deadline())
        .unwrap();
    let command = set(&graph(&store), "i01", "admit002-competing");
    println!("{CHILD_PREFIX}{}", json!({"ready":true}));
    std::io::stdout().flush().unwrap();
    let mut input = BufReader::new(std::io::stdin());
    loop {
        let mut line = String::new();
        if (&mut input).take(4097).read_line(&mut line).unwrap() == 0 {
            break;
        }
        assert!(line.len() <= 4096 && line.ends_with('\n'));
        let attempt: Value = serde_json::from_str(&line).unwrap();
        let mut input = request(&selector, command.clone(), "admit002-competing");
        if attempt["try"] == true {
            input.deadline = Instant::now();
            input.timeout_ms = 0;
        }
        let response =
            execute_direct_v1(&resolver(), input, &Clock, &NativeDirectIdSourceV1).unwrap();
        let value = match response {
            DirectResponseV1::Success(output) => output.as_value().clone(),
            DirectResponseV1::Failure(error) => error.into_value(),
        };
        println!("{CHILD_PREFIX}{value}");
        std::io::stdout().flush().unwrap();
    }
}

struct Admit002BoundNative<F> {
    calls: AtomicUsize,
    on_bound: F,
}
impl<F: Fn() + Send + Sync> podway_git::GitResolverContractV1 for Admit002BoundNative<F> {
    fn resolve(
        &self,
        selector: WorktreeSelectorV1,
    ) -> Result<podway_git::ValidatedWorktreeV1, podway_git::GitResolveErrorV1> {
        let native = NativeGitResolverV1::new().resolve(selector)?;
        if self.calls.fetch_add(1, Ordering::SeqCst) == 1 {
            (self.on_bound)();
        }
        Ok(native)
    }
}

#[test]
fn admit002_bound_revalidation_holds_guards_and_one_snapshot_with_a_real_writer() {
    use std::os::unix::fs::MetadataExt;
    for cold in [false, true] {
        let (temporary, selector, store) = prepare_fixture();
        let original = graph(&store);
        let before_sequence = sequence(&store);
        let competitor = Competitor::with_child(
            temporary.path(),
            1,
            "int_v2dfc006_races::admit002_competing_writer_child",
        );
        assert_eq!(
            competitor
                .output
                .recv_timeout(Duration::from_secs(4))
                .unwrap(),
            json!({"ready":true})
        );
        let competitor = Arc::new(Mutex::new(competitor));
        if cold {
            std::fs::remove_file(store.layout().shm()).unwrap();
        }
        let images = || {
            let mut files: Vec<_> = std::fs::read_dir(store.layout().runtime_directory())
                .unwrap()
                .map(|entry| {
                    let entry = entry.unwrap();
                    let meta = entry.metadata().unwrap();
                    (
                        entry.file_name(),
                        meta.dev(),
                        meta.ino(),
                        std::fs::read(entry.path()).unwrap(),
                    )
                })
                .collect();
            files.sort();
            files
        };
        let after_competitor = Mutex::new(None);
        let native = Admit002BoundNative {
            calls: AtomicUsize::new(0),
            on_bound: || {
                let error = store.read(Instant::now(), |_| Ok(())).unwrap_err();
                if cold {
                    assert!(
                        matches!(
                            error,
                            podway_store::direct_store::DirectStoreErrorV1::Maintenance
                        ),
                        "{error:?}"
                    );
                } else {
                    assert!(
                        matches!(
                            error,
                            podway_store::direct_store::DirectStoreErrorV1::ConnectionBusy
                        ),
                        "{error:?}"
                    );
                }
                let result = competitor
                    .lock()
                    .unwrap()
                    .commit_request(json!({"try":cold}));
                if cold {
                    assert_eq!(result["code"], "WORKSPACE_MAINTENANCE");
                    assert_eq!(result["exit_code"], 4);
                    assert!(!store.layout().shm().exists());
                } else {
                    assert_eq!(
                        result["workspace"]["latest_workspace_sequence"],
                        before_sequence + 1
                    );
                }
                *after_competitor.lock().unwrap() = Some(images());
            },
        };
        let composed = DirectWorkspaceResolverV1::new(native, digest());
        let output = success(
            execute_direct_v1(
                &composed,
                request(
                    &selector,
                    DirectCommandV1::Read {
                        command: DirectReadCommandV1::Status {
                            tier: podway_engine::v2_read_service::GraphStatusTierV2::Standard,
                            history_before: None,
                        },
                        expected_session_id: None,
                    },
                    "admit002-coherent-read",
                ),
                &Clock,
                &NativeDirectIdSourceV1,
            )
            .unwrap(),
        );
        assert_eq!(
            output["workspace"]["latest_workspace_sequence"],
            before_sequence
        );
        assert_eq!(
            output["result"]["session"]["revision"],
            original.trace().revision().get()
        );
        assert_eq!(
            images(),
            after_competitor.into_inner().unwrap().unwrap(),
            "the composed read wrote files"
        );
        let mut competitor = competitor.lock().unwrap();
        if cold {
            // Success proves the exclusive lease was released before return.
            let committed = competitor.commit_request(json!({"try":false}));
            assert_eq!(
                committed["workspace"]["latest_workspace_sequence"],
                before_sequence + 1
            );
        }
        competitor.finish();
        assert_eq!(sequence(&store), before_sequence + 1);
        assert!(outcome(&store, "admit002-competing").is_some());
        let next = success(
            execute_direct_v1(
                &resolver(),
                request(
                    &selector,
                    DirectCommandV1::Read {
                        command: DirectReadCommandV1::Status {
                            tier: podway_engine::v2_read_service::GraphStatusTierV2::Standard,
                            history_before: None,
                        },
                        expected_session_id: None,
                    },
                    "admit002-next-read",
                ),
                &Clock,
                &NativeDirectIdSourceV1,
            )
            .unwrap(),
        );
        assert_eq!(
            next["workspace"]["latest_workspace_sequence"],
            before_sequence + 1
        );
        assert_eq!(
            next["result"]["session"]["revision"],
            original.trace().revision().get() + 1
        );
    }
}
