//! Real SIGKILL recovery at current and historical Direct Engine boundaries.

use super::{
    int_v2dfc006_direct_execution::{Clock, deadline, digest},
    int_v2dfc006_runtime_ports::{Engine, NoClock},
    int_v2dfc006_workspace::{
        NoEffects, fixture as successor_fixture, mutation as successor_initialize_request,
        selector as successor_selector,
    },
};
use podway_core::{
    AttemptId, ItemId, Revision, SessionId, WorkspaceId, canonicalize_json_v1,
    execution_groups::{ExecutionCursorFenceV1, ScopedAttemptV1},
};
use podway_engine::{
    direct_api::{DirectCommandV1, DirectRequestV1, DirectResponseV1, execute_direct_v1},
    direct_execution::{
        DirectIdSourceV1, DirectReadCommandV1, NativeDirectIdSourceV1, OutcomeLookupRequestV1,
        lookup_outcome_v1,
    },
    direct_items::DirectItemCommandV1,
    direct_maintenance::{
        DirectMaintenanceBoundaryV1, DirectMaintenanceRequestV1, reset_all_v1,
        reset_all_with_boundary_observer_v1,
    },
    direct_start::DirectStartCommandV1,
    direct_workspace::initialize_workspace_v1,
    execution::ExecutionClockV1,
    successor_execution::{
        DirectSuccessorMutationV1, DirectSuccessorWriteV1, SuccessorCursor, SuccessorCursorMutation,
    },
    workspace::DirectWorkspaceResolverV1,
};
use podway_git::{
    DiagnosticPathDisplayV1, LosslessPathV1, NativeGitResolverV1, WORKTREE_SELECTOR_VERSION_V1,
    WorktreeSelectorV1,
};
use podway_protocol::{
    ItemMutationPreconditionsWireV1, ItemSetV1, RequestIdV1, Rfc3339MillisV1,
    SessionStartCommandV1, SessionStartSourceV1, SessionStartV1,
};
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
        generation: podway_protocol::ContractGenerationV1::Generation1,
        return_observation: false,
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
    let successor = input["writer"] == 16;
    let resolver = if successor {
        DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest())
    } else {
        DirectWorkspaceResolverV1::for_v15_fixture(NativeGitResolverV1::new(), digest())
    };
    let boundary = input["boundary"].as_u64().unwrap();
    if successor {
        assert!(boundary < 4, "successor item fixture uses only DE01–DE04");
    }
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
        let request = if successor {
            successor_item_request(&input, ORIGINAL_RESPONSE)
        } else {
            request(&input)
        };
        let response =
            execute_direct_v1(&resolver, request, &Clock, &NativeDirectIdSourceV1).unwrap();
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

const SUCCESSOR_KEY: &str = "killed-successor-engine-item";
const ORIGINAL_RESPONSE: &str = "00000000-0000-4000-8000-000000000096";
const RETRY_RESPONSE: &str = "00000000-0000-4000-8000-000000000097";
const REPLAY_RESPONSE: &str = "00000000-0000-4000-8000-000000000098";

fn successor_command_request(
    input: &Value,
    command: DirectCommandV1,
    key: Option<&str>,
    response: &str,
) -> DirectRequestV1 {
    DirectRequestV1 {
        selector: successor_selector(Path::new(input["root"].as_str().unwrap())),
        expected_workspace_id: Some(
            WorkspaceId::new(input["workspace"].as_str().unwrap()).unwrap(),
        ),
        request_id: RequestIdV1::new(response).unwrap(),
        command,
        idempotency_key: key.map(|key| IdempotencyKeyV1::new(key).unwrap()),
        deadline: deadline(),
        observed_at: Rfc3339MillisV1::new("2023-11-14T22:13:20.001Z").unwrap(),
        timeout_ms: 5000,
        generation: podway_protocol::ContractGenerationV1::Generation1,
        return_observation: false,
    }
}

fn successor_item_request(input: &Value, response: &str) -> DirectRequestV1 {
    let attempt: ScopedAttemptV1 = serde_json::from_value(input["attempt"].clone()).unwrap();
    let command = DirectCommandV1::SuccessorWrite(DirectSuccessorWriteV1::Edit {
        selected_group: None,
        command: DirectItemCommandV1::Set(ItemSetV1 {
            item_id: ItemId::new("scope-summary").unwrap(),
            value: "Commit the generation-16 item exactly once.".into(),
            preconditions: ItemMutationPreconditionsWireV1 {
                expected_session_id: attempt.session().clone(),
                expected_attempt_id: attempt.attempt().clone(),
                expected_item_revision: Revision::ZERO,
            },
        }),
        attempt,
    });
    successor_command_request(input, command, Some(SUCCESSOR_KEY), response)
}

fn successor_success(
    resolver: &DirectWorkspaceResolverV1<NativeGitResolverV1>,
    request: DirectRequestV1,
    clock: &impl ExecutionClockV1,
    ids: &impl DirectIdSourceV1,
) -> Value {
    match execute_direct_v1(resolver, request, clock, ids).unwrap() {
        DirectResponseV1::Success(output) => output.into_value(),
        DirectResponseV1::Failure(error) => panic!("{}", error.as_value()),
    }
}

fn successor_observe(
    resolver: &DirectWorkspaceResolverV1<NativeGitResolverV1>,
    input: &Value,
) -> Value {
    successor_success(
        resolver,
        successor_command_request(
            input,
            DirectCommandV1::Read {
                command: DirectReadCommandV1::Observe,
                expected_session_id: Some(
                    SessionId::new(input["session"].as_str().unwrap()).unwrap(),
                ),
            },
            None,
            REPLAY_RESPONSE,
        ),
        &Clock,
        &NoEffects,
    )
}

fn successor_lookup(
    resolver: &DirectWorkspaceResolverV1<NativeGitResolverV1>,
    input: &Value,
) -> Value {
    let request = successor_item_request(input, REPLAY_RESPONSE);
    lookup_outcome_v1(
        resolver,
        OutcomeLookupRequestV1 {
            selector: request.selector,
            expected_workspace_id: request.expected_workspace_id,
            request_id: request.request_id,
            key: request.idempotency_key.unwrap(),
            deadline: request.deadline,
        },
        &Clock,
    )
    .unwrap()
    .into_value()
}

fn scope_item(observation: &Value) -> &Value {
    observation["result"]["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["item"] == "scope-summary")
        .expect("the current scoped anchor declares scope-summary")
}

#[test]
fn successor_sigkill_at_four_engine_item_boundaries_preserves_effect_once_recovery() {
    let binary = FrozenBinary::new();
    for boundary in 0..4 {
        let root = successor_fixture();
        let resolver = DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest());
        let initialized = initialize_workspace_v1(
            &resolver,
            successor_initialize_request(root.path(), "successor-crash-init"),
            false,
            &Clock,
            &NativeDirectIdSourceV1,
        )
        .unwrap();
        let mut input = json!({
            "writer": 16,
            "root": root.path(),
            "workspace": initialized.as_value()["workspace"]["uuid"],
            "boundary": boundary,
            "binary": binary.path,
            "ready": root.path().join("child-ready"),
            "timing": root.path().join("child-timing"),
        });
        let started = successor_success(
            &resolver,
            successor_command_request(
                &input,
                DirectCommandV1::Start(DirectStartCommandV1::Start(SessionStartCommandV1 {
                    start: SessionStartV1 {
                        source: SessionStartSourceV1::Preset {
                            preset: "small-change-v3".into(),
                        },
                        expected_procedure_digest: None,
                        task_title: "Native successor Engine crash boundary".into(),
                        dry_run: false,
                    },
                    preconditions: None,
                })),
                Some("successor-crash-start"),
                ORIGINAL_RESPONSE,
            ),
            &Clock,
            &NativeDirectIdSourceV1,
        );
        assert_eq!(started["result"]["session_state"], "prepared");
        input["session"] = started["session"]["id"].clone();
        successor_success(
            &resolver,
            successor_command_request(
                &input,
                DirectCommandV1::SuccessorMutation(DirectSuccessorMutationV1::Cursor {
                    actor: None,
                    cursor: SuccessorCursor {
                        execution: ExecutionCursorFenceV1 {
                            session: SessionId::new(input["session"].as_str().unwrap()).unwrap(),
                            revision: Revision::ZERO,
                            goal_revision: None,
                            attempt: None,
                        },
                        selected_group: None,
                    },
                    operation: SuccessorCursorMutation::Begin(None),
                }),
                Some("successor-crash-begin"),
                ORIGINAL_RESPONSE,
            ),
            &Clock,
            &NativeDirectIdSourceV1,
        );
        let before = successor_observe(&resolver, &input);
        let sequence = before["workspace"]["latest_workspace_sequence"]
            .as_u64()
            .unwrap();
        let revision = before["result"]["cursor"]["session_revision"]
            .as_u64()
            .unwrap();
        assert_eq!(scope_item(&before)["revision"], 0);
        assert_eq!(scope_item(&before)["recorded"], false);
        input["attempt"] = before["result"]["cursor"]["attempt"].clone();
        assert_eq!(input["attempt"]["scope"]["kind"], "base");
        drop(resolver);

        let path = root.path().join("successor-child-input.json");
        fs::write(&path, serde_json::to_vec(&input).unwrap()).unwrap();
        let mut child = spawn(&path);
        wait_ready(&mut child, &input);
        child.0.kill().unwrap();
        assert_eq!(
            child.0.wait().unwrap().signal(),
            Some(9),
            "boundary {boundary}"
        );

        let cold = DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest());
        let request = successor_item_request(&input, RETRY_RESPONSE);
        let (_, store) = cold
            .resolve_existing(
                request.selector,
                request.expected_workspace_id.as_ref(),
                deadline(),
            )
            .unwrap();
        let key = request.idempotency_key.unwrap();
        let committed = boundary == 3;
        let retained = store
            .read(deadline(), |snapshot| {
                assert_eq!(snapshot.binding().schema_version(), 16);
                assert_eq!(
                    snapshot.committed_sequence()?,
                    sequence + u64::from(committed)
                );
                snapshot.outcome(&key)
            })
            .unwrap();
        assert_eq!(retained.is_some(), committed, "boundary {boundary}");
        let recovered = successor_observe(&cold, &input);
        let lookup = successor_lookup(&cold, &input);
        assert_eq!(lookup["result"]["found"], committed, "boundary {boundary}");
        if let Some(original) = &retained {
            assert_eq!(original.command(), "item.set");
            assert_eq!(original.sequence(), sequence + 1);
            assert_eq!(
                lookup["result"]["outcome"]["response"]["request_id"],
                ORIGINAL_RESPONSE
            );
            assert_eq!(scope_item(&recovered)["recorded"], true);
            assert_eq!(scope_item(&recovered)["revision"], 1);
        } else {
            assert_eq!(recovered["result"], before["result"], "boundary {boundary}");
            assert!(lookup["result"]["outcome"].is_null());
        }

        let once = if committed {
            successor_success(
                &cold,
                successor_item_request(&input, RETRY_RESPONSE),
                &NoEffects,
                &NoEffects,
            )
        } else {
            successor_success(
                &cold,
                successor_item_request(&input, RETRY_RESPONSE),
                &Clock,
                &NativeDirectIdSourceV1,
            )
        };
        assert_eq!(
            once["request_id"],
            if committed {
                ORIGINAL_RESPONSE
            } else {
                RETRY_RESPONSE
            }
        );
        assert_eq!(once["command"], "item.set");
        assert_eq!(once["workspace"]["latest_workspace_sequence"], sequence + 1);
        assert_eq!(once["session"]["revision_after"], revision + 1);
        let bytes = canonicalize_json_v1(&once).unwrap();
        if let Some(original) = retained {
            assert_eq!(original.response().as_str(), bytes);
            assert_eq!(lookup["result"]["outcome"]["response"], once);
        }
        assert_eq!(
            successor_success(
                &cold,
                successor_item_request(&input, REPLAY_RESPONSE),
                &NoEffects,
                &NoEffects,
            ),
            once,
        );
        assert_eq!(
            successor_lookup(&cold, &input)["result"]["outcome"]["response"],
            once
        );

        let final_state = successor_observe(&cold, &input);
        assert_eq!(
            final_state["workspace"]["latest_workspace_sequence"],
            sequence + 1
        );
        assert_eq!(
            final_state["result"]["cursor"]["session_revision"],
            revision + 1
        );
        assert_eq!(final_state["result"]["cursor"]["attempt"], input["attempt"]);
        assert_eq!(scope_item(&final_state)["revision"], 1);
        assert_eq!(scope_item(&final_state)["recorded"], true);
        store
            .read(deadline(), |snapshot| {
                assert_eq!(snapshot.committed_sequence()?, sequence + 1);
                let stored = snapshot
                    .outcome(&key)?
                    .expect("one committed original-key outcome");
                assert_eq!(stored.response().as_str(), bytes);
                Ok(())
            })
            .unwrap();
        store.verify_integrity(deadline()).unwrap();
    }
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
