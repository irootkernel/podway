//! G5 native maintenance exclusion with actual Engine APIs and owned OS processes.

use super::{
    int_v2dfc006_direct_execution::{Clock, deadline, digest},
    int_v2dfc006_runtime_ports::{Engine, NoClock},
    int_v2dfc006_scale::grown_l_engine,
};
use podway_core::Revision;
use podway_engine::{
    direct_api::{
        DirectCommandV1, DirectRequestV1, DirectResponseV1, execute_direct_with_error_observer_v1,
    },
    direct_execution::NativeDirectIdSourceV1,
    direct_maintenance::{
        DirectMaintenanceBoundaryV1 as Boundary, DirectMaintenanceRequestV1, remove_workspace_v1,
        remove_workspace_with_lease_observers_v1, reset_all_v1, reset_all_with_lease_observers_v1,
    },
    direct_start::DirectStartCommandV1,
    execution::prepare_workspace_procedure_source_v2,
    workspace::DirectWorkspaceResolverV1,
};
use podway_git::{
    DiagnosticPathDisplayV1, LosslessPathV1, NativeGitResolverV1, WORKTREE_SELECTOR_VERSION_V1,
    WorktreeSelectorV1,
};
use podway_protocol::{
    ProcedureV2MutationCommandV1, RequestIdV1, Rfc3339MillisV1, SessionBeginV1,
    SessionIdentityPreconditionsWireV1, SessionStartCommandV1, SessionStartSourceV1,
    SessionStartV1, validate_daemon_free_document_v1,
};
use podway_store::{
    IdempotencyKeyV1, ValidatedWorkspaceRootV1,
    direct_store::{
        DirectStoreErrorV1, DirectTransactionBoundaryV1, DirectWorkspaceLayoutV1,
        observe_direct_maintenance_marker_v1,
    },
};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{BufRead, BufReader, Write},
    os::unix::{ffi::OsStrExt, process::ExitStatusExt},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{Arc, Mutex, mpsc},
    time::{Duration, Instant},
};

const CHILD: &str = "int_v2dfc006_exclusion::maintenance_exclusion_child";
const INPUT: &str = "PODWAY_ENGINE_G5_INPUT";
const PREFIX: &str = "V2DFC006_G5 ";
const RESET_STEPS: [&str; 7] = [
    "intent",
    "marker",
    "runtime-cleared",
    "runtime-removed",
    "store-created",
    "reset-marker-removed",
    "intent-removed",
];
const REMOVE_STEPS: [&str; 8] = [
    "intent",
    "marker",
    "runtime-cleared",
    "runtime-removed",
    "configuration-removed",
    "marker-first",
    "marker-last",
    "directories-removed",
];

fn resolver() -> DirectWorkspaceResolverV1<NativeGitResolverV1> {
    DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest())
}
fn selector(root: &Path) -> WorktreeSelectorV1 {
    WorktreeSelectorV1::new(
        WORKTREE_SELECTOR_VERSION_V1,
        None,
        LosslessPathV1::from_raw_bytes(
            root.as_os_str().as_bytes(),
            DiagnosticPathDisplayV1::new(root.to_string_lossy()).unwrap(),
        )
        .unwrap(),
    )
    .unwrap()
}
fn request(root: &Path, command: DirectCommandV1, key: &str) -> DirectRequestV1 {
    DirectRequestV1 {
        selector: selector(root),
        expected_workspace_id: None,
        request_id: RequestIdV1::new("00000000-0000-4000-8000-000000000099").unwrap(),
        command,
        idempotency_key: Some(IdempotencyKeyV1::new(key).unwrap()),
        deadline: deadline(),
        observed_at: Rfc3339MillisV1::new("2023-11-14T22:13:20.001Z").unwrap(),
        timeout_ms: 5000,
        generation: podway_protocol::ContractGenerationV1::Generation1,
    }
}
fn maintenance(input: &Value) -> DirectMaintenanceRequestV1 {
    DirectMaintenanceRequestV1 {
        selector: selector(Path::new(input["root"].as_str().unwrap())),
        expected_workspace_id: None,
        request_id: RequestIdV1::new("00000000-0000-4000-8000-000000000098").unwrap(),
        key: IdempotencyKeyV1::new(input["key"].as_str().unwrap()).unwrap(),
        confirmed: true,
        force: input["kind"] == "remove",
        deadline: Instant::now() + Duration::from_secs(30),
    }
}
fn append(file: &mut File, value: &Value) {
    writeln!(file, "{}", serde_json::to_string(value).unwrap()).unwrap();
    file.flush().unwrap();
}
fn send(value: Value) {
    println!("{PREFIX}{}", serde_json::to_string(&value).unwrap());
    std::io::stdout().flush().unwrap();
}
fn phase(directory: &Path, value: &str) {
    let pending = directory.join("phase.pending");
    fs::write(&pending, value).unwrap();
    fs::rename(pending, directory.join("phase")).unwrap();
}
fn current_phase(directory: &Path) -> String {
    fs::read_to_string(directory.join("phase")).unwrap_or_default()
}
fn quiesce_writers(directory: &Path, value: &str, token: &str) {
    phase(directory, value);
    fs::write(directory.join("pause-token"), token).unwrap();
    let limit = Instant::now() + Duration::from_secs(40);
    loop {
        if (0..4).all(|writer| {
            fs::read_to_string(directory.join(format!("paused-{writer}")))
                .ok()
                .as_deref()
                == Some(token)
        }) {
            return;
        }
        assert!(
            Instant::now() < limit,
            "writers did not acknowledge the bounded seeding pause"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn marker(root: &Path) -> Value {
    let layout = DirectWorkspaceLayoutV1::new(
        ValidatedWorkspaceRootV1::from_unix_bytes(root.as_os_str().as_bytes().to_vec()).unwrap(),
    )
    .unwrap();
    match observe_direct_maintenance_marker_v1(&layout) {
        Ok(Some(marker)) => {
            let document: Value = serde_json::from_str(marker.document().as_str()).unwrap();
            json!({"operation":document["operation_id"],"command":document["command"],"phase":document["phase"]})
        }
        Ok(None) => Value::Null,
        Err(error) => json!({"observation_error":format!("{error:?}")}),
    }
}
fn document(response: DirectResponseV1) -> Result<Value, Value> {
    match response {
        DirectResponseV1::Success(output) => {
            assert!(validate_daemon_free_document_v1(
                "podway.output/v4",
                output.as_value()
            ));
            Ok(output.as_value().clone())
        }
        DirectResponseV1::Failure(error) => {
            let value = error.into_value();
            assert!(validate_daemon_free_document_v1("podway.error/v2", &value));
            Err(value)
        }
    }
}
fn step(boundary: Boundary, root: &Path) -> Option<String> {
    match boundary {
        Boundary::IntentPublished => Some("intent".into()),
        Boundary::MarkerPublished => Some("marker".into()),
        Boundary::RuntimeCleared => Some("runtime-cleared".into()),
        Boundary::PhaseAdvanced => marker(root)["phase"]
            .as_str()
            .map(|phase| phase.replace('_', "-")),
        Boundary::ResetMarkerRemoved => Some("reset-marker-removed".into()),
        Boundary::IntentMarkerRemoved => Some("intent-removed".into()),
        Boundary::AfterFirstRemovalMarker => Some("marker-first".into()),
        Boundary::AfterLastRemovalMarker => Some("marker-last".into()),
        Boundary::DirectoriesRemoved => Some("directories-removed".into()),
        _ => None,
    }
}

struct OwnedChild {
    child: Child,
    input: Option<ChildStdin>,
    messages: mpsc::Receiver<Value>,
    diagnostic: PathBuf,
}
impl OwnedChild {
    fn spawn(directory: &Path, spec: Value, name: &str) -> Self {
        let input = directory.join(format!("{name}.json"));
        fs::write(&input, serde_json::to_vec(&spec).unwrap()).unwrap();
        let mut child = Command::new(directory.join("engine-child"))
            .args(["--exact", CHILD, "--nocapture", "--test-threads=1"])
            .env(INPUT, input)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().unwrap();
        let (tx, messages) = mpsc::channel();
        let diagnostic = directory.join(format!("child-{name}.log"));
        let mut output = File::create(&diagnostic).unwrap();
        std::thread::spawn(move || {
            let mut retained = 0;
            for line in BufReader::new(stdout).lines() {
                let line = line.unwrap();
                assert!(line.len() <= 65536);
                if retained + line.len() < 131072 {
                    writeln!(output, "{line}").unwrap();
                    output.flush().unwrap();
                    retained += line.len() + 1;
                }
                if let Some((_, json)) = line.split_once(PREFIX)
                    && tx.send(serde_json::from_str(json).unwrap()).is_err()
                {
                    break;
                }
            }
        });
        Self {
            child,
            input: stdin,
            messages,
            diagnostic,
        }
    }
    fn message(&self) -> Value {
        self.messages
            .recv_timeout(Duration::from_secs(40))
            .unwrap_or_else(|error| {
                panic!(
                    "owned G5 child missed bounded barrier: {error}; stdout: {}",
                    fs::read_to_string(&self.diagnostic).unwrap()
                )
            })
    }
    fn go(&mut self) {
        writeln!(self.input.as_mut().unwrap(), "go").unwrap();
        self.input.as_mut().unwrap().flush().unwrap();
    }
    fn killed(&mut self) {
        self.child.kill().unwrap();
        assert_eq!(self.child.wait().unwrap().signal(), Some(9));
    }
    fn finish(&mut self) {
        let limit = Instant::now() + Duration::from_secs(40);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success());
                break;
            }
            assert!(Instant::now() < limit);
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}
impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn maintenance_exclusion_child() {
    let Some(input) = std::env::var_os(INPUT) else {
        return;
    };
    let bytes = fs::read(input).unwrap();
    assert!(bytes.len() <= 32768);
    let input: Value = serde_json::from_slice(&bytes).unwrap();
    let root = PathBuf::from(input["root"].as_str().unwrap());
    let directory = PathBuf::from(input["directory"].as_str().unwrap());
    assert!(root.join(".git").is_dir());
    if input["role"] == "operation" {
        let kind = input["kind"].as_str().unwrap().to_owned();
        let wanted = input["step"].as_str().unwrap().to_owned();
        let expected = maintenance(&input);
        let initial_uuid = input["initial_uuid"].as_str().unwrap().to_owned();
        let regular = {
            let root = root.clone();
            let directory = directory.clone();
            move |boundary| {
                if boundary == Boundary::BeforeFinalRemoval {
                    phase(&directory, "finalizing");
                }
                if boundary == Boundary::AfterLastRemovalMarker {
                    phase(&directory, "absent");
                }
                if step(boundary, &root).as_deref() == Some(wanted.as_str()) {
                    send(json!({"event":"paused","step":wanted}));
                    loop {
                        std::thread::sleep(Duration::from_secs(1));
                    }
                }
            }
        };
        let qualified = {
            let directory = directory.clone();
            move |boundary,
                  store: &podway_store::direct_store::DirectStoreV1,
                  lease: &podway_store::direct_store::ExclusiveMaintenanceLeaseV1| {
                match boundary {
                    Boundary::LeaseAcquired { wait } => {
                        send(json!({"event":"acquired","wait_us":wait.as_micros()}))
                    }
                    Boundary::BeforeRuntimeDestruction => {
                        let snapshot=store.read_under_exclusive_lease(lease,Instant::now()+Duration::from_secs(30),|snapshot|{
                    let uuid=snapshot.binding().identity().workspace_uuid().as_str().to_owned();
                    let mut outcomes=Vec::new();
                    for key in snapshot.retained_outcome_keys_for_qualification_v1(10_000)? {
                        let outcome=snapshot.outcome(&key)?.expect("qualified listed outcome disappeared");
                        outcomes.push(json!({"key":key.as_str(),"sequence":outcome.sequence(),
                            "response":serde_json::from_str::<Value>(outcome.response().as_str()).unwrap()}));
                    }
                    let nodes=snapshot.current_graph()?.map(|graph|graph.snapshot().graph_nodes().len());
                    Ok(json!({"uuid":uuid,"outcomes":outcomes,"nodes":nodes}))
                });
                        match snapshot {
                            Ok(snapshot) => {
                                assert_eq!(snapshot["nodes"], 300);
                                let uuid = snapshot["uuid"].as_str().unwrap();
                                let path = directory.join(format!("retired-{uuid}.json"));
                                if path.exists() {
                                    assert_eq!(
                                        serde_json::from_slice::<Value>(&fs::read(&path).unwrap())
                                            .unwrap(),
                                        snapshot
                                    );
                                } else {
                                    fs::write(path, serde_json::to_vec(&snapshot).unwrap())
                                        .unwrap();
                                }
                            }
                            Err(DirectStoreErrorV1::NotInitialized) => assert!(
                                directory
                                    .join(format!("retired-{initial_uuid}.json"))
                                    .exists()
                            ),
                            Err(error) => panic!("qualified retirement snapshot failed: {error:?}"),
                        }
                    }
                    _ => panic!("unexpected lease boundary"),
                }
            }
        };
        let output = if kind == "reset" {
            reset_all_with_lease_observers_v1(
                &resolver(),
                expected,
                &Clock,
                &NativeDirectIdSourceV1,
                regular,
                qualified,
            )
        } else {
            remove_workspace_with_lease_observers_v1(
                &resolver(),
                expected,
                &Clock,
                regular,
                qualified,
            )
        }
        .unwrap();
        send(json!({"event":"done","output":output.as_value()}));
        return;
    }
    let writer = input["writer"].as_u64().unwrap();
    let reader = input["reader"].as_bool().unwrap();
    let mut log = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(directory.join(format!("contender-{writer}.jsonl")))
        .unwrap();
    send(json!({"event":"ready"}));
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).unwrap();
    assert_eq!(line, "go\n");
    let mut counts = BTreeMap::<String, u64>::new();
    let mut writes = BTreeMap::<String, u64>::new();
    let mut attempts = 0;
    while !directory.join("stop").exists() {
        if !reader && matches!(current_phase(&directory).as_str(), "seeding" | "recreating") {
            if let Ok(token) = fs::read(directory.join("pause-token")) {
                fs::write(directory.join(format!("paused-{writer}")), token).unwrap();
            }
            std::thread::sleep(Duration::from_millis(2));
            continue;
        }
        attempts += 1;
        assert!(attempts < 1_000_000);
        let before_phase = current_phase(&directory);
        let before_marker = marker(&root);
        let typed = RefCell::new(Vec::new());
        let failure_observation = RefCell::new(Value::Null);
        let missing = Arc::new(Mutex::new(Vec::<Value>::new()));
        let observed_resolver = {
            let missing = missing.clone();
            let directory = directory.clone();
            resolver().with_store_boundary_observer_v1(move |boundary| {
                if boundary == DirectTransactionBoundaryV1::UnmarkedMissingReadClassified {
                    missing.lock().unwrap().push(
                        json!({"classification":"missing","maintenance_marked":false,
                        "phase":current_phase(&directory)}),
                    );
                }
            })
        };
        let observe = |error: &podway_engine::direct_execution::DirectExecutionErrorV1| {
            typed.borrow_mut().push(format!("{error:?}"));
            *failure_observation.borrow_mut() =
                json!({"marker":marker(&root),"phase":current_phase(&directory)});
        };
        let shown = document(
            execute_direct_with_error_observer_v1(
                &observed_resolver,
                request(&root, DirectCommandV1::WorkspaceShow, "unused"),
                &Clock,
                &NativeDirectIdSourceV1,
                observe,
            )
            .unwrap(),
        );
        let result = match shown {
            Ok(output) if !reader => {
                let observed = output["workspace"]["uuid"].as_str().unwrap();
                if writes.get(observed).copied().unwrap_or(0) >= 20 {
                    std::thread::sleep(Duration::from_millis(2));
                    continue;
                }
                document(
                    execute_direct_with_error_observer_v1(
                        &observed_resolver,
                        request(
                            &root,
                            DirectCommandV1::WorkspaceRepair,
                            &format!("g5-w{writer}-{attempts}"),
                        ),
                        &Clock,
                        &NativeDirectIdSourceV1,
                        observe,
                    )
                    .unwrap(),
                )
            }
            other => other,
        };
        let after_marker = marker(&root);
        let after_phase = current_phase(&directory);
        let class = match result {
            Ok(output) => {
                if !reader {
                    let uuid = output["workspace"]["uuid"].as_str().unwrap().to_owned();
                    *writes.entry(uuid).or_default() += 1;
                    append(
                        &mut log,
                        &json!({"ack":output,"key":format!("g5-w{writer}-{attempts}")}),
                    );
                }
                if reader {
                    "read".to_owned()
                } else {
                    "success".to_owned()
                }
            }
            Err(error) => {
                let code = error["code"].as_str().unwrap().to_owned();
                append(
                    &mut log,
                    &json!({"error":error,"before_marker":before_marker,"after_marker":after_marker,
                    "before_phase":before_phase,"after_phase":after_phase,"typed":typed.into_inner(),
                    "failure_observation":failure_observation.into_inner(),"missing_classifications":missing.lock().unwrap().clone()}),
                );
                code
            }
        };
        *counts.entry(class).or_default() += 1;
        std::thread::sleep(Duration::from_millis(2));
    }
    send(json!({"event":"done","counts":counts,"writes":writes}));
}

fn operation(
    engine: &Engine,
    directory: &Path,
    kind: &str,
    cycle: usize,
    acquisitions: &mut Vec<u64>,
) -> Value {
    let uuid = engine
        .store
        .read(deadline(), |snapshot| {
            Ok(snapshot.binding().identity().workspace_uuid().clone())
        })
        .unwrap();
    let key = format!("g5-{kind}-{cycle}");
    let steps = if kind == "reset" {
        RESET_STEPS.as_slice()
    } else {
        REMOVE_STEPS.as_slice()
    };
    let mut last = None;
    for (ordinal, target) in steps
        .iter()
        .copied()
        .chain(std::iter::once("finish"))
        .enumerate()
    {
        let spec = json!({"role":"operation","root":engine.temporary.path(),"directory":directory,"kind":kind,
            "key":key,"step":target,"initial_uuid":uuid});
        let mut child = OwnedChild::spawn(
            directory,
            spec,
            &format!("operation-{kind}-{cycle}-{ordinal}"),
        );
        loop {
            let message = child.message();
            match message["event"].as_str().unwrap() {
                "acquired" => acquisitions.push(message["wait_us"].as_u64().unwrap()),
                "paused" => {
                    assert_eq!(message["step"], target);
                    child.killed();
                    break;
                }
                "done" => {
                    assert_eq!(
                        target, "finish",
                        "requested marker boundary was not reached"
                    );
                    last = Some(message["output"].clone());
                    child.finish();
                    break;
                }
                event => panic!("unexpected operation event {event}"),
            }
        }
    }
    let last = last.unwrap();
    let spec = json!({"root":engine.temporary.path(),"key":key,"kind":kind});
    let replay = if kind == "reset" {
        reset_all_v1(
            &resolver(),
            maintenance(&spec),
            &NoClock,
            &NativeDirectIdSourceV1,
        )
        .unwrap()
    } else {
        remove_workspace_v1(&resolver(), maintenance(&spec), &Clock).unwrap()
    };
    if kind == "reset" {
        assert_eq!(replay.as_value(), &last);
        assert_ne!(last["workspace"]["uuid"], json!(uuid));
    } else {
        assert_eq!(last["result"]["already_absent"], true);
        assert_eq!(replay.as_value()["result"]["already_absent"], true);
        assert!(!engine.temporary.path().join(".podway").exists());
    }
    last
}
fn seed(engine: &Engine, source: &str, generation: usize) {
    engine.success(
        DirectCommandV1::Start(DirectStartCommandV1::Start(SessionStartCommandV1 {
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
                task_title: "G5 fresh same Procedure".into(),
                dry_run: false,
            },
            preconditions: None,
        })),
        &format!("g5-seed-start-{generation}"),
    );
    let graph = engine.graph();
    engine.success(
        DirectCommandV1::Procedure(ProcedureV2MutationCommandV1::SessionBegin(SessionBeginV1 {
            initial_goal: None,
            preconditions: SessionIdentityPreconditionsWireV1 {
                expected_session_id: graph.trace().session_id().clone(),
                expected_session_revision: Revision::ZERO,
            },
        })),
        &format!("g5-seed-begin-{generation}"),
    );
    payloads(engine, generation);
}
fn payloads(engine: &Engine, generation: usize) {
    assert_eq!(engine.graph().snapshot().graph_nodes().len(), 300);
    for writer in 0..4 {
        engine.set(
            &format!("item-{writer}"),
            &"x".repeat(64),
            &format!("g5-set-{generation}-{writer}"),
        );
    }
}

#[test]
#[ignore = "explicit release-profile G5 qualification: 100 cycles, 4 writers +4 readers, SIGKILL at every marker step"]
fn g5_100_reset_remove_cycles_preserve_all_acknowledged_namespaces_with_four_writers_and_readers() {
    let evidence = PathBuf::from(
        std::env::var_os("PODWAY_ENGINE_G5_EVIDENCE_ROOT")
            .expect("explicit G5 qualification requires its owned evidence root"),
    );
    let directory = evidence.join("harness");
    fs::create_dir(&directory).unwrap();
    let directory = directory.as_path();
    fs::copy(
        std::env::current_exe().unwrap(),
        directory.join("engine-child"),
    )
    .unwrap();
    let binary_sha256 = format!(
        "sha256:{:x}",
        Sha256::digest(fs::read(directory.join("engine-child")).unwrap())
    );
    let (engine, (grown, fresh, _nodes)) = grown_l_engine(300);
    assert!(grown >= 43_384_832 && fresh >= 43_384_832);
    let source = fs::read_to_string(engine.temporary.path().join("source.yaml")).unwrap();
    phase(directory, "seeding");
    payloads(&engine, 0);
    let mut children: Vec<_> = (0..8)
        .map(|writer| {
            OwnedChild::spawn(
                directory,
                json!({"role":"contender","writer":writer,"reader":writer>=4,
        "root":engine.temporary.path(),"directory":directory}),
                &format!("contender-{writer}"),
            )
        })
        .collect();
    for child in &children {
        assert_eq!(child.message()["event"], "ready");
    }
    for child in &mut children {
        child.go();
    }
    let mut acquisitions = Vec::new();
    for cycle in 0..100 {
        phase(directory, "live");
        std::thread::sleep(Duration::from_millis(40));
        phase(directory, "maintenance");
        operation(&engine, directory, "reset", cycle, &mut acquisitions);
        quiesce_writers(directory, "seeding", &format!("reset-{cycle}"));
        seed(&engine, &source, cycle * 2 + 1);
        phase(directory, "live");
        std::thread::sleep(Duration::from_millis(40));
        phase(directory, "maintenance");
        operation(&engine, directory, "remove", cycle, &mut acquisitions);
        quiesce_writers(directory, "recreating", &format!("remove-{cycle}"));
        engine.success(
            DirectCommandV1::WorkspaceInit { repair: false },
            &format!("g5-init-{cycle}"),
        );
        phase(directory, "seeding");
        seed(&engine, &source, cycle * 2 + 2);
    }
    fs::write(directory.join("stop"), b"stop").unwrap();
    let mut counts = BTreeMap::<String, u64>::new();
    for child in &mut children {
        let report = child.message();
        assert_eq!(report["event"], "done");
        child.finish();
        for (class, count) in report["counts"].as_object().unwrap() {
            *counts.entry(class.clone()).or_default() += count.as_u64().unwrap();
        }
    }
    let mut retained = BTreeMap::<String, BTreeMap<String, Value>>::new();
    for entry in fs::read_dir(directory).unwrap() {
        let entry = entry.unwrap();
        if entry.file_name().to_string_lossy().starts_with("retired-") {
            let snapshot: Value = serde_json::from_slice(&fs::read(entry.path()).unwrap()).unwrap();
            retained.insert(
                snapshot["uuid"].as_str().unwrap().into(),
                snapshot["outcomes"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|row| {
                        (
                            row["key"].as_str().unwrap().to_owned(),
                            row["response"].clone(),
                        )
                    })
                    .collect(),
            );
        }
    }
    assert_eq!(retained.len(), 200);
    let current = engine
        .store
        .read(deadline(), |snapshot| {
            let uuid = snapshot
                .binding()
                .identity()
                .workspace_uuid()
                .as_str()
                .to_owned();
            let mut outcomes = BTreeMap::new();
            for key in snapshot.retained_outcome_keys_for_qualification_v1(10_000)? {
                outcomes.insert(
                    key.as_str().to_owned(),
                    serde_json::from_str::<Value>(
                        snapshot.outcome(&key)?.unwrap().response().as_str(),
                    )
                    .unwrap(),
                );
            }
            Ok((uuid, outcomes))
        })
        .unwrap();
    retained.insert(current.0, current.1);
    let mut acknowledgments = 0;
    let mut lost = 0;
    let mut marked_missing = 0;
    let mut unexplained_missing = 0;
    for writer in 0..8 {
        for line in
            BufReader::new(File::open(directory.join(format!("contender-{writer}.jsonl"))).unwrap())
                .lines()
        {
            let row: Value = serde_json::from_str(&line.unwrap()).unwrap();
            if !row["ack"].is_null() {
                acknowledgments += 1;
                let uuid = row["ack"]["workspace"]["uuid"].as_str().unwrap();
                let key = row["key"].as_str().unwrap();
                if retained.get(uuid).and_then(|keys| keys.get(key)) != Some(&row["ack"]) {
                    lost += 1;
                }
            }
            if row["error"]["code"] == "WORKSPACE_NOT_INITIALIZED" {
                let marked = !row["before_marker"]["operation"].is_null()
                    && row["before_marker"]["operation"] == row["after_marker"]["operation"];
                if marked {
                    marked_missing += 1;
                }
                let classifications = row["missing_classifications"].as_array().unwrap();
                let in_unmarked_window = classifications.len() == 1
                    && classifications[0]["maintenance_marked"] == false
                    && classifications[0]["classification"] == "missing";
                // The classifier proves the exact unmarked Missing point. The
                // controller's later phase read is supporting provenance only.
                if !in_unmarked_window {
                    unexplained_missing += 1;
                }
            }
        }
    }
    let permitted: BTreeSet<&str> = [
        "success",
        "read",
        "WORKSPACE_MAINTENANCE",
        "WORKSPACE_NOT_INITIALIZED",
    ]
    .into_iter()
    .collect();
    let unexpected: u64 = counts
        .iter()
        .filter(|(class, _)| !permitted.contains(class.as_str()))
        .map(|(_, count)| *count)
        .sum();
    acquisitions.sort_unstable();
    let p99 = acquisitions[(acquisitions.len() * 99).div_ceil(100) - 1];
    let summary = json!({"cycles":100,"writers":4,"readers":4,"killed_attempts":100*(RESET_STEPS.len()+REMOVE_STEPS.len()),
        "reset_steps":RESET_STEPS,"removal_steps":REMOVE_STEPS,
        "test_binary_sha256":binary_sha256,
        "initial_grown_bytes":grown,"initial_fresh_bytes":fresh,"repair_cap_per_writer_uuid":20,
        "acknowledgments":acknowledgments,"lost":lost,"marked_missing":marked_missing,"unexplained_missing":unexplained_missing,
        "outcomes":counts,"unexpected":unexpected,"acquisition_samples":acquisitions.len(),"acquisition_p99_us":p99});
    fs::write(
        directory.join("result.json"),
        serde_json::to_vec_pretty(&summary).unwrap(),
    )
    .unwrap();
    fs::write(
        directory.join("acquisitions.json"),
        serde_json::to_vec(&acquisitions).unwrap(),
    )
    .unwrap();
    println!("G5 actual Engine {summary}");
    assert!(acknowledgments > 0);
    assert_eq!(
        (lost, marked_missing, unexplained_missing, unexpected),
        (0, 0, 0, 0)
    );
    assert!(p99 <= 2_000_000);
    engine.store.verify_integrity(deadline()).unwrap();
}
