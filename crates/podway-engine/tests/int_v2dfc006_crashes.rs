//! Real SIGKILL recovery at direct Engine transaction and reset publication boundaries.

use super::{
    int_v2dfc006_direct_execution::{Clock, deadline, digest},
    int_v2dfc006_runtime_ports::{Engine, NoClock},
};
use podway_core::{AttemptId, ItemId, Revision, SessionId, WorkspaceId};
use podway_engine::{
    direct_api::{DirectCommandV1, DirectRequestV1, DirectResponseV1, execute_direct_v1},
    direct_execution::NativeDirectIdSourceV1,
    direct_items::DirectItemCommandV1,
    direct_maintenance::{
        DirectMaintenanceBoundaryV1, DirectMaintenanceRequestV1, reset_all_v1,
        reset_all_with_boundary_observer_v1,
    },
    workspace::DirectWorkspaceResolverV1,
};
use podway_git::{
    DiagnosticPathDisplayV1, LosslessPathV1, NativeGitResolverV1, WORKTREE_SELECTOR_VERSION_V1,
    WorktreeSelectorV1,
};
use podway_protocol::{ItemMutationPreconditionsWireV1, ItemSetV1, RequestIdV1, Rfc3339MillisV1};
use podway_store::{IdempotencyKeyV1, direct_store::DirectTransactionBoundaryV1};
use serde_json::{Value, json};
use std::{
    fs,
    os::unix::process::ExitStatusExt,
    path::Path,
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

const CHILD_TEST: &str = "int_v2dfc006_crashes::engine_crash_child";
const CHILD_ENV: &str = "PODWAY_ENGINE_CRASH_INPUT";

fn procedure() -> String {
    let mut source = String::from(
        "schema: podway.procedure/v2\nid: engine-crash\nversion: \"1\"\nname: Engine crash\npurpose: Qualify effect-once Engine recovery.\nnode_definitions:\n  work:\n    type: action\n    title: Work\n    intent: Record payloads.\n    items:\n",
    );
    for n in 0..16 {
        source.push_str(&format!("      - id: item-{n}\n        type: text\n        prompt: Record payload.\n        required: false\n        max_length: 65536\n"));
    }
    source.push_str("graph:\n  entry: n0\n  nodes:\n");
    for n in 0..8 {
        source.push_str(&format!("    - id: n{n}\n      use: work\n"));
        if n == 7 {
            source.push_str("      terminal: true\n");
        } else {
            source.push_str(&format!("      next: n{}\n", n + 1));
        }
    }
    source
}

fn command(input: &Value) -> DirectCommandV1 {
    DirectCommandV1::Item(DirectItemCommandV1::Set(ItemSetV1 {
        item_id: ItemId::new("item-0").unwrap(),
        value: "x".repeat(64),
        preconditions: ItemMutationPreconditionsWireV1 {
            expected_session_id: SessionId::new(input["session"].as_str().unwrap()).unwrap(),
            expected_attempt_id: AttemptId::new(input["attempt"].as_str().unwrap()).unwrap(),
            expected_item_revision: Revision::ZERO,
        },
    }))
}
fn request(input: &Value) -> DirectRequestV1 {
    DirectRequestV1 {
        selector: WorktreeSelectorV1::new(
            WORKTREE_SELECTOR_VERSION_V1,
            None,
            LosslessPathV1::from_raw_bytes(
                input["root"].as_str().unwrap().as_bytes(),
                DiagnosticPathDisplayV1::new(input["root"].as_str().unwrap()).unwrap(),
            )
            .unwrap(),
        )
        .unwrap(),
        expected_workspace_id: None,
        request_id: RequestIdV1::new("00000000-0000-4000-8000-000000000099").unwrap(),
        command: command(input),
        idempotency_key: Some(IdempotencyKeyV1::new("killed-engine-mutation").unwrap()),
        deadline: deadline(),
        observed_at: Rfc3339MillisV1::new("2023-11-14T22:13:20.001Z").unwrap(),
        timeout_ms: 5000,
    }
}
fn reset_request(input: &Value) -> DirectMaintenanceRequestV1 {
    DirectMaintenanceRequestV1 {
        selector: request(input).selector,
        expected_workspace_id: Some(
            WorkspaceId::new(input["workspace"].as_str().unwrap()).unwrap(),
        ),
        request_id: request(input).request_id,
        key: IdempotencyKeyV1::new("killed-engine-reset").unwrap(),
        confirmed: true,
        force: false,
        deadline: deadline(),
    }
}
fn rendezvous(path: &Path) -> ! {
    fs::write(path, b"ready").unwrap();
    loop {
        std::thread::sleep(Duration::from_secs(1));
    }
}

#[test]
fn engine_crash_child() {
    let Some(path) = std::env::var_os(CHILD_ENV) else {
        return;
    };
    let input: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
    let ready = std::path::PathBuf::from(input["ready"].as_str().unwrap());
    let resolver = DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest());
    let boundary = input["boundary"].as_u64().unwrap();
    if boundary == 4 {
        reset_all_with_boundary_observer_v1(
            &resolver,
            reset_request(&input),
            &Clock,
            &NativeDirectIdSourceV1,
            move |actual| {
                if actual
                    == DirectMaintenanceBoundaryV1::StoreBoundary(
                        DirectTransactionBoundaryV1::BeforeMaintenancePublication,
                    )
                {
                    rendezvous(&ready)
                }
            },
        )
        .unwrap();
    } else {
        let resolver = resolver.with_store_boundary_observer_v1(move |actual| {
            let expected = match boundary {
                0 => DirectTransactionBoundaryV1::BeforeBegin,
                1 => DirectTransactionBoundaryV1::AfterBegin,
                2 => DirectTransactionBoundaryV1::AfterStateBeforeOutcome,
                3 => DirectTransactionBoundaryV1::AfterCommitBeforeOutput,
                _ => return,
            };
            if actual == expected {
                rendezvous(&ready)
            }
        });
        if boundary == 5 {
            fs::write(input["ready"].as_str().unwrap(), b"ready").unwrap();
        }
        let began = Instant::now();
        let response =
            execute_direct_v1(&resolver, request(&input), &Clock, &NativeDirectIdSourceV1).unwrap();
        assert!(matches!(response, DirectResponseV1::Success(_)));
        fs::write(
            input["timing"].as_str().unwrap(),
            began.elapsed().as_micros().to_string(),
        )
        .unwrap();
    }
}

struct OwnedChild(Child);
impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
fn spawn(input: &Path) -> OwnedChild {
    let document: Value = serde_json::from_slice(&fs::read(input).unwrap()).unwrap();
    OwnedChild(
        Command::new(document["binary"].as_str().unwrap())
            .args(["--exact", CHILD_TEST, "--nocapture"])
            .env(CHILD_ENV, input)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    )
}
struct FrozenBinary {
    _temporary: tempfile::TempDir,
    path: std::path::PathBuf,
}
impl FrozenBinary {
    fn new() -> Self {
        let temporary = tempfile::Builder::new()
            .prefix("engine-crash-binary-")
            .tempdir()
            .unwrap();
        let path = temporary.path().join("qualification-child");
        fs::copy(std::env::current_exe().unwrap(), &path).unwrap();
        Self {
            _temporary: temporary,
            path,
        }
    }
}
fn input(engine: &Engine, boundary: u64, binary: &FrozenBinary) -> Value {
    let graph = engine.graph();
    json!({"root":engine.temporary.path(),"session":graph.trace().session_id(),
        "attempt":graph.trace().active_attempt().unwrap().attempt_id(),
        "workspace":engine.store.read(deadline(),|snapshot|Ok(snapshot.binding().identity().workspace_uuid().clone())).unwrap(),
        "binary":binary.path,"boundary":boundary,"ready":engine.temporary.path().join("child-ready"),"timing":engine.temporary.path().join("child-timing")})
}
fn wait_ready(child: &mut OwnedChild, input: &Value) {
    let ready = Path::new(input["ready"].as_str().unwrap());
    let limit = deadline();
    while !ready.exists() {
        assert!(
            child.0.try_wait().unwrap().is_none(),
            "owned Engine child exited before rendezvous"
        );
        assert!(Instant::now() < limit, "owned Engine rendezvous expired");
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn recover(engine: &mut Engine, input: &Value, committed: Option<bool>) -> bool {
    engine.cold();
    let sequence = engine.sequence();
    let key = IdempotencyKeyV1::new("killed-engine-mutation").unwrap();
    let retained = engine
        .store
        .read(deadline(), |snapshot| snapshot.outcome(&key))
        .unwrap();
    let applied = retained.is_some();
    if let Some(expected) = committed {
        assert_eq!(applied, expected);
    }
    assert_eq!(sequence, 3 + u64::from(applied));
    let once = if applied {
        engine.output(engine.execute(request(input), &NoClock))
    } else {
        engine.output(engine.execute(request(input), &Clock))
    };
    assert_eq!(engine.sequence(), 4);
    assert_eq!(
        engine.output(engine.execute(request(input), &NoClock)),
        once
    );
    assert_eq!(engine.sequence(), 4);
    engine.store.verify_integrity(deadline()).unwrap();
    applied
}

#[test]
#[ignore = "explicit native G4 Engine kill qualification"]
fn sigkill_at_all_five_engine_boundaries_preserves_effect_once_recovery() {
    let binary = FrozenBinary::new();
    for boundary in 0..5 {
        for _ in 0..20 {
            let mut engine = Engine::new(&procedure());
            let input = input(&engine, boundary, &binary);
            let path = engine.temporary.path().join("child-input.json");
            fs::write(&path, serde_json::to_vec(&input).unwrap()).unwrap();
            let mut child = spawn(&path);
            wait_ready(&mut child, &input);
            child.0.kill().unwrap();
            assert_eq!(child.0.wait().unwrap().signal(), Some(9));
            if boundary == 4 {
                engine.cold();
                assert!(
                    engine
                        .resolver
                        .resolve_existing(engine.selector.clone(), None, deadline())
                        .is_err()
                );
                let once = reset_all_v1(
                    &engine.resolver,
                    reset_request(&input),
                    &NoClock,
                    &NativeDirectIdSourceV1,
                )
                .unwrap();
                assert_eq!(
                    reset_all_v1(
                        &engine.resolver,
                        reset_request(&input),
                        &NoClock,
                        &NativeDirectIdSourceV1
                    )
                    .unwrap(),
                    once
                );
                let (_, store) = engine
                    .resolver
                    .resolve_existing(engine.selector.clone(), None, deadline())
                    .unwrap();
                store
                    .read(deadline(), |snapshot| {
                        assert!(snapshot.current_graph()?.is_none());
                        assert_eq!(snapshot.committed_sequence()?, 1);
                        Ok(())
                    })
                    .unwrap();
                store.verify_integrity(deadline()).unwrap();
            } else {
                recover(&mut engine, &input, Some(boundary == 3));
            }
        }
    }
}

#[test]
#[ignore = "explicit native G4 200 live random Engine kills"]
fn random_sigkills_preserve_success_only_lookup_and_exact_retry() {
    let binary = FrozenBinary::new();
    let mut samples = Vec::new();
    for _ in 0..20 {
        let engine = Engine::new(&procedure());
        let input = input(&engine, 5, &binary);
        let path = engine.temporary.path().join("input.json");
        fs::write(&path, serde_json::to_vec(&input).unwrap()).unwrap();
        let mut child = spawn(&path);
        assert!(child.0.wait().unwrap().success());
        samples.push(
            fs::read_to_string(input["timing"].as_str().unwrap())
                .unwrap()
                .parse::<u64>()
                .unwrap(),
        );
    }
    samples.sort_unstable();
    let median = samples[samples.len() / 2];
    assert!(median > 0);
    let mut random = 0x9e3779b97f4a7c15u64;
    let mut killed = 0;
    let mut attempts = 0;
    let mut precommit = 0;
    while killed < 200 {
        attempts += 1;
        assert!(attempts <= 2000, "live random kill admission exhausted");
        let mut engine = Engine::new(&procedure());
        let input = input(&engine, 5, &binary);
        let path = engine.temporary.path().join("input.json");
        fs::write(&path, serde_json::to_vec(&input).unwrap()).unwrap();
        let mut child = spawn(&path);
        wait_ready(&mut child, &input);
        random ^= random << 13;
        random ^= random >> 7;
        random ^= random << 17;
        std::thread::sleep(Duration::from_micros(random % (median + 1)));
        if child.0.try_wait().unwrap().is_some() {
            continue;
        }
        if child.0.kill().is_err() {
            continue;
        }
        if child.0.wait().unwrap().signal() != Some(9) {
            continue;
        }
        killed += 1;
        precommit += usize::from(!recover(&mut engine, &input, None));
    }
    println!(
        "ENGINE_G4_RANDOM {}",
        json!({"killed":killed,"attempts":attempts,"precommit":precommit,"median_writer_us":median})
    );
    assert_eq!(killed, 200);
}

#[test]
#[ignore = "explicit release-profile G3 500 ms Engine deadline qualification"]
fn engine_write_deadlines_return_busy_without_effect_or_outcome() {
    let binary = FrozenBinary::new();
    let mut engine = Engine::new(&procedure());
    let input = input(&engine, 1, &binary);
    let path = engine.temporary.path().join("deadline-controller.json");
    fs::write(&path, serde_json::to_vec(&input).unwrap()).unwrap();
    let mut child = spawn(&path);
    wait_ready(&mut child, &input);
    let before = engine.graph();
    let sequence = engine.sequence();
    let mut measured = Vec::new();
    for n in 0..220 {
        let key = IdempotencyKeyV1::new(format!("engine-deadline-{n}")).unwrap();
        let mut attempt = request(&input);
        attempt.idempotency_key = Some(key.clone());
        attempt.timeout_ms = 500;
        let began = Instant::now();
        attempt.deadline = began + Duration::from_millis(500);
        let DirectResponseV1::Failure(error) = engine.execute(attempt, &NoClock) else {
            panic!("locked mutation committed")
        };
        let elapsed = began.elapsed();
        let error = error.into_value();
        assert_eq!(error["code"], "WORKSPACE_BUSY");
        assert_eq!(error["details"]["boundary"], "sqlite_write_lock");
        assert_eq!(error["details"]["timeout_ms"], 500);
        assert_eq!(error["retryable"], true);
        assert!(
            elapsed <= Duration::from_millis(600),
            "500 ms deadline exceeded grace: {elapsed:?}"
        );
        if n >= 20 {
            measured.push(elapsed.as_micros() as u64);
        }
        engine
            .store
            .read(deadline(), |snapshot| {
                assert_eq!(snapshot.current_graph()?.as_ref(), Some(&before));
                assert_eq!(snapshot.committed_sequence()?, sequence);
                assert!(snapshot.outcome(&key)?.is_none());
                Ok(())
            })
            .unwrap();
    }
    child.0.kill().unwrap();
    assert_eq!(child.0.wait().unwrap().signal(), Some(9));
    recover(&mut engine, &input, Some(false));
    measured.sort_unstable();
    assert_eq!(measured.len(), 200);
    let metrics = json!({"warmups":20,"measured":200,"timeout_ms":500,"grace_ms":100,
        "latency_us":{"p50":measured[99],"p95":measured[189],"p99":measured[197],"maximum":measured[199]},
        "partial_effects":0,"rejected_outcomes":0});
    if let Some(path) = std::env::var_os("PODWAY_ENGINE_G3_REPORT") {
        use std::io::Write as _;
        let mut report = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .unwrap();
        report
            .write_all(serde_json::to_string_pretty(&metrics).unwrap().as_bytes())
            .unwrap();
    }
    println!("ENGINE_G3 {metrics}");
}
