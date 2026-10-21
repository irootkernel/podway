//! The public failure boundary validates substantive domain details and recovery identity.

use podway_core::{
    AttemptId, CriterionId, DomainError, GoalRevisionNumberV2, GraphNodeId, ItemId, Revision,
    SessionId, SessionLifecycle, Sha256Digest,
};
use podway_engine::{
    direct_error::{DirectErrorContextV1, render_direct_error_v1},
    direct_execution::{DirectDelegationFailureV1, DirectExecutionErrorV1},
    execution::ProcedureV2StartPreparationErrorV1,
};
use podway_protocol::{
    CommandNameV1, RequestIdV1, Rfc3339MillisV1, validate_daemon_free_document_v1,
};
use podway_store::{
    GraphMutationErrorV2, IdempotencyKeyV1, StoreErrorV1, direct_store::DirectStoreErrorV1,
};

const ID: &str = "00000000-0000-4000-8000-000000000001";
const OTHER: &str = "00000000-0000-4000-8000-000000000002";

// Forward successful native values unchanged: rebuilding them would discard
// transient Git-object evidence and fail before the intended admission stage.
struct Admit001Native {
    calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    at: usize,
    fail: bool,
    replacement: Option<(std::path::PathBuf, std::path::PathBuf)>,
}

impl podway_git::GitResolverContractV1 for Admit001Native {
    fn resolve(
        &self,
        selector: podway_git::WorktreeSelectorV1,
    ) -> Result<podway_git::ValidatedWorktreeV1, podway_git::GitResolveErrorV1> {
        use std::{os::unix::fs::MetadataExt, sync::atomic::Ordering};
        let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
        let native = podway_git::NativeGitResolverV1::new().resolve(selector)?;
        if call == self.at {
            if let Some((path, saved)) = &self.replacement {
                std::fs::rename(path, saved).unwrap();
                std::fs::copy(saved, path).unwrap();
                assert_ne!(
                    std::fs::metadata(path).unwrap().ino(),
                    std::fs::metadata(saved).unwrap().ino()
                );
            }
            if self.fail {
                return Err(podway_git::GitResolveErrorV1::WorktreeDeleted);
            }
        }
        Ok(native)
    }
}

struct Admit001Clock {
    calls: std::sync::atomic::AtomicUsize,
    allowed: bool,
}

impl podway_engine::execution::ExecutionClockV1 for Admit001Clock {
    fn now(&self) -> podway_core::UnixMillis {
        assert!(self.allowed, "admission failure read the clock");
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        podway_core::UnixMillis::new(1_700_000_000_001)
    }
}

fn admit001_read_error<R: podway_git::GitResolverContractV1>(
    route: &str,
    resolver: &podway_engine::workspace::DirectWorkspaceResolverV1<R>,
    selector: &podway_git::WorktreeSelectorV1,
    workspace: Option<podway_core::WorkspaceId>,
    clock: &impl podway_engine::execution::ExecutionClockV1,
) -> DirectExecutionErrorV1 {
    use super::{
        int_v2dfc006_direct_execution::deadline, int_v2dfc006_races::reads004_facts_request,
    };
    use podway_engine::direct_execution::{
        DirectReadCommandV1, DirectReadRequestV1, OutcomeLookupRequestV1, execute_read_v1,
        lookup_outcome_v1, preflight_facts_v1,
    };
    match route {
        "status" => execute_read_v1(
            resolver,
            DirectReadRequestV1 {
                request_id: RequestIdV1::new(ID).unwrap(),
                selector: selector.clone(),
                expected_workspace_id: workspace,
                expected_session_id: None,
                command: DirectReadCommandV1::Status {
                    tier: podway_engine::v2_read_service::GraphStatusTierV2::Standard,
                    history_before: None,
                },
                deadline: deadline(),
            },
            clock,
        )
        .map(|_| ()),
        "preflight" => {
            let mut request = reads004_facts_request(selector);
            request.expected_workspace_id = workspace;
            preflight_facts_v1(resolver, request, clock).map(|_| ())
        }
        "lookup" => lookup_outcome_v1(
            resolver,
            OutcomeLookupRequestV1 {
                selector: selector.clone(),
                expected_workspace_id: workspace,
                request_id: RequestIdV1::new(ID).unwrap(),
                key: IdempotencyKeyV1::new("init-key").unwrap(),
                deadline: deadline(),
            },
            clock,
        )
        .map(|_| ()),
        _ => unreachable!(),
    }
    .unwrap_err()
}

fn admit001_corrupt_slot(store: &podway_store::direct_store::DirectStoreV1, attempt: &AttemptId) {
    assert_eq!(
        podway_store::test_support::corrupt_recorded_slot_value(
            store.layout().database(),
            attempt,
            "i01",
            "{\"kind\":\"mystery\"}",
        )
        .unwrap(),
        1
    );
}

fn admit001_runtime_bytes(
    store: &podway_store::direct_store::DirectStoreV1,
) -> std::collections::BTreeMap<std::ffi::OsString, Vec<u8>> {
    std::fs::read_dir(store.layout().runtime_directory())
        .unwrap()
        .map(|entry| {
            let entry = entry.unwrap();
            (entry.file_name(), std::fs::read(entry.path()).unwrap())
        })
        .collect()
}

#[test]
fn admit001_resolver_and_operation_failures_keep_order_and_wrappers() {
    use super::{
        int_v2dfc006_direct_execution::{deadline, digest},
        int_v2dfc006_races::{graph, prepare_fixture},
    };
    use podway_engine::{
        direct_workspace::{
            DirectWorkspaceErrorV1, DirectWorkspaceReadRequestV1, show_workspace_v1,
        },
        workspace::{
            DirectWorkspaceResolutionErrorV1 as Resolution, DirectWorkspaceResolverV1,
            WorkspaceGitObservationV1, WorkspaceResolutionErrorV1,
        },
    };
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    for route in ["status", "preflight", "lookup", "show"] {
        for stage in [
            "preliminary",
            "qualification",
            "uuid",
            "bound",
            "operation",
            "view",
        ] {
            if route == "lookup" && stage == "view" {
                continue; // Outcome lookup deliberately does not load the graph.
            }
            let (_root, selector, store) = prepare_fixture();
            let current = graph(&store);
            admit001_corrupt_slot(
                &store,
                current.trace().active_attempt().unwrap().attempt_id(),
            );
            if stage == "qualification" {
                rusqlite::Connection::open(store.layout().database())
                    .unwrap()
                    .execute(
                        "UPDATE writer_contract SET contract_manifest_digest=?1 WHERE singleton=1",
                        [format!("sha256:{}", "f".repeat(64))],
                    )
                    .unwrap();
            }
            let before = admit001_runtime_bytes(&store);
            let calls = Arc::new(AtomicUsize::new(0));
            let at = match stage {
                "preliminary" => 1,
                "uuid" | "bound" => 2,
                "operation" => 3,
                _ => 0,
            };
            let resolver = DirectWorkspaceResolverV1::new(
                Admit001Native {
                    calls: Arc::clone(&calls),
                    at,
                    fail: true,
                    replacement: None,
                },
                digest(),
            );
            let workspace = matches!(stage, "preliminary" | "qualification" | "uuid")
                .then(|| podway_core::WorkspaceId::new(OTHER).unwrap());
            let clock = Admit001Clock {
                calls: AtomicUsize::new(0),
                allowed: stage == "view",
            };
            let error = if route == "show" {
                let result = show_workspace_v1(
                    &resolver,
                    DirectWorkspaceReadRequestV1 {
                        selector: selector.clone(),
                        expected_workspace_id: workspace,
                        request_id: RequestIdV1::new(ID).unwrap(),
                        deadline: deadline(),
                    },
                    &clock,
                );
                if stage == "view" {
                    // Show has no view: even corrupt graph bytes must not add admission.
                    result.unwrap();
                    assert_eq!(clock.calls.load(Ordering::SeqCst), 1);
                    assert_eq!(admit001_runtime_bytes(&store), before);
                    continue;
                }
                let error = result.unwrap_err();
                assert!(
                    matches!(error, DirectWorkspaceErrorV1::Resolution(_)),
                    "{stage}: {error:?}"
                );
                DirectExecutionErrorV1::from(error)
            } else {
                admit001_read_error(route, &resolver, &selector, workspace, &clock)
            };
            match stage {
                "qualification" => assert!(
                    matches!(
                        error,
                        DirectExecutionErrorV1::Workspace(Resolution::Store(
                            DirectStoreErrorV1::WriterMismatch { .. }
                        ))
                    ),
                    "{route}/{stage}: {error:?}"
                ),
                "uuid" => assert!(
                    matches!(
                        error,
                        DirectExecutionErrorV1::Workspace(Resolution::Resolution(
                            WorkspaceResolutionErrorV1::ExpectedWorkspaceUuidMismatch { .. }
                        ))
                    ),
                    "{route}/{stage}: {error:?}"
                ),
                "view" => assert!(
                    matches!(
                        error,
                        DirectExecutionErrorV1::Store(DirectStoreErrorV1::State(
                            StoreErrorV1::CorruptStateV1 { .. }
                        ))
                    ),
                    "{route}/{stage}: {error:?}"
                ),
                _ => {
                    let expected = if stage == "preliminary" {
                        WorkspaceGitObservationV1::Preliminary
                    } else {
                        WorkspaceGitObservationV1::BoundRevalidation
                    };
                    assert!(
                        matches!(error, DirectExecutionErrorV1::Workspace(
                        Resolution::Resolution(WorkspaceResolutionErrorV1::Git {
                            observation, source: podway_git::GitResolveErrorV1::WorktreeDeleted,
                        })
                    ) if observation == expected),
                        "{route}/{stage}: {error:?}"
                    );
                }
            }
            assert_eq!(
                calls.load(Ordering::SeqCst),
                match stage {
                    "preliminary" | "qualification" | "uuid" => 1,
                    "bound" => 2,
                    _ => 3,
                },
                "{route}/{stage}"
            );
            assert_eq!(
                clock.calls.load(Ordering::SeqCst),
                usize::from(stage == "view")
            );
            assert_eq!(admit001_runtime_bytes(&store), before);
        }
    }
}

#[test]
fn admit001_operation_witness_precedes_carried_native_and_view_errors() {
    use super::{
        int_v2dfc006_direct_execution::{deadline, digest},
        int_v2dfc006_races::{graph, prepare_fixture},
    };
    use podway_engine::{
        direct_workspace::{
            DirectWorkspaceErrorV1, DirectWorkspaceReadRequestV1, show_workspace_v1,
        },
        workspace::{DirectWorkspaceResolutionErrorV1, DirectWorkspaceResolverV1},
    };
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    for route in ["status", "preflight", "show"] {
        for failure in ["native", "view"] {
            if route == "show" && failure == "view" {
                continue;
            }
            for witness in ["none", "database", "lock"] {
                let (root, selector, store) = prepare_fixture();
                let current = graph(&store);
                admit001_corrupt_slot(
                    &store,
                    current.trace().active_attempt().unwrap().attempt_id(),
                );
                let before = admit001_runtime_bytes(&store);
                let path = match witness {
                    "none" => None,
                    "database" => Some(store.layout().database()),
                    "lock" => Some(store.layout().maintenance_lock()),
                    _ => unreachable!(),
                };
                let calls = Arc::new(AtomicUsize::new(0));
                let resolver = DirectWorkspaceResolverV1::new(
                    Admit001Native {
                        calls: Arc::clone(&calls),
                        at: 3,
                        fail: failure == "native",
                        replacement: path
                            .map(|path| (path, root.path().join("owned-operation-witness"))),
                    },
                    digest(),
                );
                let clock = Admit001Clock {
                    calls: AtomicUsize::new(0),
                    allowed: failure == "view",
                };
                let error = if route == "show" {
                    let error = show_workspace_v1(
                        &resolver,
                        DirectWorkspaceReadRequestV1 {
                            selector: selector.clone(),
                            expected_workspace_id: None,
                            request_id: RequestIdV1::new(ID).unwrap(),
                            deadline: deadline(),
                        },
                        &clock,
                    )
                    .unwrap_err();
                    if witness == "none" {
                        assert!(
                            matches!(error, DirectWorkspaceErrorV1::Resolution(_)),
                            "{error:?}"
                        );
                    } else {
                        assert!(
                            matches!(
                                error,
                                DirectWorkspaceErrorV1::Store(DirectStoreErrorV1::Maintenance)
                            ),
                            "{error:?}"
                        );
                    }
                    DirectExecutionErrorV1::from(error)
                } else {
                    admit001_read_error(route, &resolver, &selector, None, &clock)
                };
                if witness != "none" {
                    assert!(
                        matches!(
                            error,
                            DirectExecutionErrorV1::Store(DirectStoreErrorV1::Maintenance)
                        ),
                        "{route}/{failure}/{witness}: {error:?}"
                    );
                } else if failure == "native" {
                    assert!(
                        matches!(
                            error,
                            DirectExecutionErrorV1::Workspace(
                                DirectWorkspaceResolutionErrorV1::Resolution(_)
                            )
                        ),
                        "{route}: {error:?}"
                    );
                } else {
                    assert!(
                        matches!(
                            error,
                            DirectExecutionErrorV1::Store(DirectStoreErrorV1::State(
                                StoreErrorV1::CorruptStateV1 { .. }
                            ))
                        ),
                        "{route}: {error:?}"
                    );
                }
                assert_eq!(calls.load(Ordering::SeqCst), 3);
                assert_eq!(
                    clock.calls.load(Ordering::SeqCst),
                    usize::from(failure == "view")
                );
                assert_eq!(admit001_runtime_bytes(&store), before);
            }
        }
    }
}

#[test]
fn admit001_cold_preparation_rejects_unstamped_post_preflight_corruption() {
    use super::{
        int_v2dfc006_direct_execution::{Clock, deadline, digest},
        int_v2dfc006_races::{
            cursor_fences, graph, item_fences, prepare_fixture, reads004_facts_request,
        },
    };
    use podway_engine::{
        direct_api::{DirectCommandV1, DirectRequestV1, DirectResponseV1, execute_direct_v1},
        direct_execution::{
            DirectCursorCommandV1, DirectCursorRequestV1, NativeDirectIdSourceV1,
            execute_cursor_v1, preflight_facts_v1,
        },
        direct_items::DirectItemCommandV1,
        workspace::DirectWorkspaceResolverV1,
    };
    use podway_protocol::{ItemSetV1, SessionCancelV1, SessionCompleteV1};
    use podway_store::direct_store::DirectTransactionBoundaryV1;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let (_root, selector, store) = prepare_fixture();
    let resolver = DirectWorkspaceResolverV1::new(podway_git::NativeGitResolverV1::new(), digest());
    let initial = graph(&store);
    let historical_attempt = initial
        .trace()
        .active_attempt()
        .unwrap()
        .attempt_id()
        .clone();
    let execute = |command, key: &str| {
        let response = execute_direct_v1(
            &resolver,
            DirectRequestV1 {
                selector: selector.clone(),
                expected_workspace_id: None,
                request_id: RequestIdV1::new(ID).unwrap(),
                command,
                idempotency_key: Some(IdempotencyKeyV1::new(key).unwrap()),
                deadline: deadline(),
                observed_at: Rfc3339MillisV1::new("2023-11-14T22:13:20.001Z").unwrap(),
                timeout_ms: 5000,
            },
            &Clock,
            &NativeDirectIdSourceV1,
        )
        .unwrap();
        assert!(
            matches!(response, DirectResponseV1::Success(_)),
            "{response:?}"
        );
    };
    execute(
        DirectCommandV1::Item(DirectItemCommandV1::Set(ItemSetV1 {
            item_id: ItemId::new("i01").unwrap(),
            value: "historical evidence".into(),
            preconditions: item_fences(&initial, "i01"),
        })),
        "admit001-history",
    );
    execute(
        DirectCommandV1::Cursor(DirectCursorCommandV1::Complete(SessionCompleteV1 {
            preconditions: cursor_fences(&graph(&store)),
        })),
        "admit001-advance",
    );
    let current = graph(&store);
    assert_ne!(
        current.trace().active_attempt().unwrap().attempt_id(),
        &historical_attempt
    );
    let facts = preflight_facts_v1(&resolver, reads004_facts_request(&selector), &Clock).unwrap();
    assert_eq!(facts.session_revision, current.trace().revision());
    // These are exactly the comparison stamps, plus the persisted locator.
    // The graph's historical payload changes without advancing any of them.
    let stamps = || {
        let connection = rusqlite::Connection::open(store.layout().database()).unwrap();
        connection.query_row(
            "SELECT w.last_validated_root,c.contract_manifest_digest,c.generation_id,w.workspace_uuid,\
             CAST(w.next_workspace_sequence AS TEXT),s.session_id,CAST(s.session_revision AS TEXT),\
             CAST(v.workspace_revision AS TEXT),s.procedure_snapshot_id,p.digest \
             FROM workspace_state w JOIN writer_contract c ON c.singleton=w.singleton \
             JOIN v2_workspace_state v ON v.singleton=w.singleton \
             JOIN v2_task_sessions s ON s.session_id=v.current_session_id \
             JOIN v2_procedure_snapshots p ON p.snapshot_id=s.procedure_snapshot_id WHERE w.singleton=1",
            [], |row| (0..10).map(|index| row.get::<_, String>(index)).collect::<rusqlite::Result<Vec<_>>>(),
        ).unwrap()
    };
    let before_stamps = stamps();
    admit001_corrupt_slot(&store, &historical_attempt);
    assert_eq!(stamps(), before_stamps);
    let before = admit001_runtime_bytes(&store);
    let begins = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&begins);
    let resolver = resolver.with_store_boundary_observer_v1(move |boundary| {
        if boundary == DirectTransactionBoundaryV1::BeforeBegin {
            observed.fetch_add(1, Ordering::SeqCst);
        }
    });
    let key = IdempotencyKeyV1::new("admit001-cold-corruption").unwrap();
    let result = execute_cursor_v1(
        &resolver,
        DirectCursorRequestV1 {
            request_id: RequestIdV1::new(OTHER).unwrap(),
            selector,
            expected_workspace_id: Some(facts.workspace_id),
            key: key.clone(),
            command: DirectCursorCommandV1::Cancel(SessionCancelV1 {
                reason: "Retain the cold preparation boundary.".into(),
                preconditions: cursor_fences(&current),
            }),
            deadline: deadline(),
        },
        &Admit001Clock {
            calls: AtomicUsize::new(0),
            allowed: false,
        },
        &NativeDirectIdSourceV1,
    );
    assert!(
        matches!(
            result,
            Err(DirectExecutionErrorV1::Store(DirectStoreErrorV1::State(
                StoreErrorV1::CorruptStateV1 { .. }
            )))
        ),
        "{result:?}"
    );
    assert_eq!(begins.load(Ordering::SeqCst), 0);
    assert_eq!(admit001_runtime_bytes(&store), before);
    assert_eq!(stamps(), before_stamps);
    store
        .read(deadline(), |snapshot| {
            assert!(snapshot.outcome(&key)?.is_none());
            Ok(())
        })
        .unwrap();
}

fn context(command: &str) -> DirectErrorContextV1 {
    DirectErrorContextV1 {
        request_id: RequestIdV1::new(ID).unwrap(),
        command: CommandNameV1::new(command).unwrap(),
        observed_at: Rfc3339MillisV1::new("2023-11-14T22:13:20.000Z").unwrap(),
        idempotency_key: Some(IdempotencyKeyV1::new("original-key").unwrap()),
        timeout_ms: 5000,
        maintenance: None,
    }
}

fn assert_error(error: DirectExecutionErrorV1, command: &str, code: &str) -> serde_json::Value {
    let output = render_direct_error_v1(&error, &context(command))
        .unwrap_or_else(|failure| panic!("{code} ({error:?}) did not validate: {failure:?}"));
    let value = output.into_value();
    assert_eq!(value["code"], code);
    assert!(validate_daemon_free_document_v1("podway.error/v2", &value));
    if let Some(recovery) = value["details"].get("recovery") {
        assert!(validate_daemon_free_document_v1(
            "podway.recovery-recipe/v2",
            recovery
        ));
    }
    assert!(value.get("job").is_none());
    assert!(value["details"].get("admission").is_none());
    value
}

#[test]
fn direct_contention_errors_identify_the_actual_admission_boundary() {
    for (error, command, boundary) in [
        (DirectStoreErrorV1::Busy, "item.set", "sqlite_write_lock"),
        (
            DirectStoreErrorV1::ConnectionBusy,
            "session.status",
            "connection_gate",
        ),
        (
            DirectStoreErrorV1::ReadAdmissionBusy,
            "session.status",
            "sqlite_read_admission",
        ),
    ] {
        let value = assert_error(
            DirectExecutionErrorV1::Store(error),
            command,
            "WORKSPACE_BUSY",
        );
        assert_eq!(value["details"]["boundary"], boundary);
        assert_eq!(value["details"]["timeout_ms"], 5000);
        assert_eq!(value["retryable"], true);
        assert_eq!(value["exit_code"], 4);
    }
}

#[test]
fn actual_connection_gate_contention_returns_a_valid_public_error() {
    use std::{
        os::unix::fs::MetadataExt,
        sync::{Arc, Mutex, mpsc},
        time::{Duration, Instant},
    };

    use podway_engine::{
        direct_api::{DirectCommandV1, DirectRequestV1, DirectResponseV1, execute_direct_v1},
        direct_execution::NativeDirectIdSourceV1,
        workspace::DirectWorkspaceResolverV1,
    };
    use podway_git::NativeGitResolverV1;
    use podway_store::{
        CanonicalExecutionJsonV1,
        direct_store::{
            DirectMutationRequestV1, DirectOutcomeScopeV1, DirectTransactionBoundaryV1,
        },
    };

    use super::int_v2dfc006_direct_execution::{Clock, deadline, digest, fixture};

    let (_root, selector, store, output) = fixture();
    let (ready_tx, ready_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let release = Arc::new(Mutex::new(release_rx));
    let writer = store.clone().with_boundary_observer(move |boundary| {
        if boundary == DirectTransactionBoundaryV1::AfterCommitBeforeOutput {
            ready_tx.send(()).unwrap();
            release
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5))
                .unwrap();
        }
    });
    let child = std::thread::spawn(move || {
        let request = DirectMutationRequestV1::new(
            IdempotencyKeyV1::new("gate-writer").unwrap(),
            digest(),
            "workspace.init",
            output.as_value()["request_id"].as_str().unwrap(),
            DirectOutcomeScopeV1::Workspace,
        )
        .unwrap();
        writer.mutate(
            &request,
            deadline(),
            || podway_core::UnixMillis::new(1_700_000_000_001),
            |transaction| {
                transaction.now();
                Ok(CanonicalExecutionJsonV1::new(
                    podway_core::canonicalize_json_v1(output.as_value()).unwrap(),
                )
                .unwrap())
            },
        )
    });
    if let Err(error) = ready_rx.recv_timeout(Duration::from_secs(5)) {
        panic!(
            "writer never reached the held connection: {error}; {:?}",
            child.join().unwrap()
        );
    }
    let images = || {
        let mut files: Vec<_> = std::fs::read_dir(store.layout().runtime_directory())
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                let metadata = entry.metadata().unwrap();
                (
                    entry.file_name(),
                    metadata.dev(),
                    metadata.ino(),
                    std::fs::read(entry.path()).unwrap(),
                )
            })
            .collect();
        files.sort();
        files
    };
    let before = images();
    let resolver = DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest());
    let responses = [0_u64, 20].map(|timeout| {
        let started = Instant::now();
        let response = execute_direct_v1(
            &resolver,
            DirectRequestV1 {
                request_id: RequestIdV1::new(ID).unwrap(),
                selector: selector.clone(),
                expected_workspace_id: None,
                command: DirectCommandV1::WorkspaceShow,
                idempotency_key: None,
                deadline: started + Duration::from_millis(timeout),
                observed_at: Rfc3339MillisV1::new("2023-11-14T22:13:20.001Z").unwrap(),
                timeout_ms: timeout,
            },
            &Clock,
            &NativeDirectIdSourceV1,
        );
        (timeout, response, started.elapsed())
    });
    let after = images();
    release_tx.send(()).unwrap();
    assert!(child.join().unwrap().is_ok());
    assert_eq!(after, before);
    for (timeout, response, elapsed) in responses {
        let DirectResponseV1::Failure(error) = response.unwrap() else {
            panic!("read admitted while the actual writer still owns its connection");
        };
        let value = error.into_value();
        assert!(validate_daemon_free_document_v1("podway.error/v2", &value));
        assert_eq!(value["command"], "workspace.show");
        assert_eq!(value["code"], "WORKSPACE_BUSY");
        assert_eq!(value["details"]["boundary"], "connection_gate");
        assert_eq!(value["details"]["timeout_ms"], timeout);
        assert_eq!(value["exit_code"], 4);
        assert_eq!(value["retryable"], true);
        assert!(elapsed < Duration::from_secs(2));
    }
    assert_eq!(
        store
            .read(deadline(), |snapshot| snapshot.committed_sequence())
            .unwrap(),
        2
    );
}

#[test]
fn direct_failure_catalog_preserves_store_fences_domain_and_recovery() {
    let session = SessionId::new(ID).unwrap();
    let attempt = AttemptId::new(ID).unwrap();
    let sha = Sha256Digest::new(format!("sha256:{}", "a".repeat(64))).unwrap();
    let graph = |error| DirectExecutionErrorV1::Store(DirectStoreErrorV1::Domain(error));
    let store = |error| DirectExecutionErrorV1::Store(error);
    let cases = vec![
        (
            store(DirectStoreErrorV1::NotInitialized),
            "WORKSPACE_NOT_INITIALIZED",
        ),
        (store(DirectStoreErrorV1::Busy), "WORKSPACE_BUSY"),
        (store(DirectStoreErrorV1::ConnectionBusy), "WORKSPACE_BUSY"),
        (
            store(DirectStoreErrorV1::ReadAdmissionBusy),
            "WORKSPACE_BUSY",
        ),
        (
            store(DirectStoreErrorV1::Maintenance),
            "WORKSPACE_MAINTENANCE",
        ),
        (
            store(DirectStoreErrorV1::Legacy {
                generation: Some(13),
                marker: Some("reset.marker"),
            }),
            "LEGACY_RUNTIME_STATE_UNSUPPORTED",
        ),
        (
            store(DirectStoreErrorV1::Legacy {
                generation: Some(podway_store::direct_store::DIRECT_SCHEMA_GENERATION_V1 - 1),
                marker: None,
            }),
            "LEGACY_RUNTIME_STATE_UNSUPPORTED",
        ),
        (
            store(DirectStoreErrorV1::WriterMismatch {
                expected: sha.clone(),
                actual: sha.as_str().into(),
            }),
            "WORKSPACE_CONTRACT_MISMATCH",
        ),
        (
            store(DirectStoreErrorV1::UnsupportedGeneration {
                generation: podway_store::direct_store::DIRECT_SCHEMA_GENERATION_V1 + 1,
            }),
            "WORKSPACE_SCHEMA_UNSUPPORTED",
        ),
        (
            store(DirectStoreErrorV1::Unreadable),
            "WORKSPACE_STATE_UNREADABLE",
        ),
        (
            store(DirectStoreErrorV1::SchemaMismatch),
            "WORKSPACE_STATE_UNREADABLE",
        ),
        (
            store(DirectStoreErrorV1::BindingMismatch),
            "WORKSPACE_ID_CONFLICT",
        ),
        (
            store(DirectStoreErrorV1::IdempotencyKeyReused),
            "IDEMPOTENCY_KEY_REUSED",
        ),
        (
            store(DirectStoreErrorV1::MutationOutcomeUnknown),
            "MUTATION_OUTCOME_UNKNOWN",
        ),
        (
            graph(GraphMutationErrorV2::SessionRevisionConflict {
                expected: Revision::new(2),
                actual: Revision::new(3),
            }),
            "SESSION_REVISION_CONFLICT",
        ),
        (
            graph(GraphMutationErrorV2::AttemptNotCurrent {
                expected: attempt.clone(),
                actual: None,
            }),
            "ATTEMPT_NOT_CURRENT",
        ),
        (
            graph(GraphMutationErrorV2::Domain(
                DomainError::SessionIdentityMismatch {
                    expected: session.clone(),
                    actual: None,
                },
            )),
            "SESSION_ID_MISMATCH",
        ),
        (
            graph(GraphMutationErrorV2::Domain(
                DomainError::BlockerLimitReached {
                    maximum_open_blockers: 64,
                },
            )),
            "BLOCKER_LIMIT_REACHED",
        ),
        (
            graph(GraphMutationErrorV2::Domain(DomainError::ArtifactChanged)),
            "ARTIFACT_CHANGED",
        ),
        (
            graph(GraphMutationErrorV2::ItemTypeMismatch),
            "ITEM_TYPE_MISMATCH",
        ),
        (
            graph(GraphMutationErrorV2::ItemConstraintFailed),
            "ITEM_CONSTRAINT_FAILED",
        ),
        (
            graph(GraphMutationErrorV2::GoalTrackingNotEnabled),
            "GOAL_TRACKING_NOT_ENABLED",
        ),
        (
            graph(GraphMutationErrorV2::SessionGoalAlreadyDefined {
                goal_revision: GoalRevisionNumberV2::new(1),
            }),
            "SESSION_GOAL_ALREADY_DEFINED",
        ),
        (
            graph(GraphMutationErrorV2::GoalRevisionStale {
                expected: GoalRevisionNumberV2::new(1),
                actual: GoalRevisionNumberV2::new(2),
            }),
            "GOAL_REVISION_STALE",
        ),
        (
            graph(GraphMutationErrorV2::GoalRevisionTargetNotAllowed {
                target_graph_node_id: GraphNodeId::new("work").unwrap(),
            }),
            "GOAL_REVISION_TARGET_NOT_ALLOWED",
        ),
        (
            graph(GraphMutationErrorV2::GoalRevisionTargetNotRevisionSafe {
                target_graph_node_id: GraphNodeId::new("work").unwrap(),
            }),
            "GOAL_REVISION_TARGET_NOT_REVISION_SAFE",
        ),
        (
            graph(GraphMutationErrorV2::ReactivationFlagRequired),
            "REACTIVATION_FLAG_REQUIRED",
        ),
        (
            graph(GraphMutationErrorV2::CriterionNotFound {
                criterion_id: CriterionId::new("verified").unwrap(),
            }),
            "CRITERION_NOT_FOUND",
        ),
        (
            graph(GraphMutationErrorV2::CriterionCitationInvalid {
                criterion_id: CriterionId::new("verified").unwrap(),
                citation: podway_core::CriterionCitationV2::Item(ItemId::new("report").unwrap()),
            }),
            "CRITERION_CITATION_INVALID",
        ),
        (
            graph(GraphMutationErrorV2::CriterionResultMissing {
                missing_criterion_ids: vec![CriterionId::new("verified").unwrap()],
            }),
            "CRITERION_RESULT_MISSING",
        ),
        (
            graph(GraphMutationErrorV2::FreshGoalAssessmentMissing {
                goal_revision: GoalRevisionNumberV2::new(1),
            }),
            "FRESH_GOAL_ASSESSMENT_MISSING",
        ),
        (
            DirectExecutionErrorV1::Start(
                ProcedureV2StartPreparationErrorV1::DigestConfirmationRequired {
                    procedure_digest: sha.clone(),
                },
            ),
            "DIGEST_CONFIRMATION_REQUIRED",
        ),
        (
            DirectExecutionErrorV1::Start(
                ProcedureV2StartPreparationErrorV1::ProcedureDigestMismatch {
                    expected: sha.clone(),
                    actual: sha.clone(),
                },
            ),
            "PROCEDURE_DIGEST_MISMATCH",
        ),
        (
            DirectExecutionErrorV1::StartDecisionRequired {
                session_id: session,
                revision: Revision::new(1),
                lifecycle: SessionLifecycle::Running,
                current_terminal_disposition: false,
            },
            "SESSION_START_DECISION_REQUIRED",
        ),
        (
            DirectExecutionErrorV1::Delegation(DirectDelegationFailureV1::ReceiptRequired),
            "DELEGATION_RECEIPT_REQUIRED",
        ),
        (
            DirectExecutionErrorV1::Delegation(DirectDelegationFailureV1::ItemsChanged),
            "DELEGATION_RECEIPT_STALE",
        ),
        (
            DirectExecutionErrorV1::Domain(DomainError::RevisionOverflow {
                revision: Revision::new(u64::MAX),
            }),
            "INTERNAL_ERROR",
        ),
    ];
    for (error, code) in cases {
        assert_error(error, "session.complete", code);
    }
    for lifecycle in [
        SessionLifecycle::Running,
        SessionLifecycle::Completed,
        SessionLifecycle::Cancelled,
    ] {
        assert_error(
            store(DirectStoreErrorV1::State(
                StoreErrorV1::SessionResetNotEligibleV1 {
                    lifecycle,
                    current_terminal_disposition: false,
                },
            )),
            "session.reset",
            "SESSION_RESET_NOT_ELIGIBLE",
        );
    }
    for lifecycle in [
        SessionLifecycle::Prepared,
        SessionLifecycle::Running,
        SessionLifecycle::Completed,
        SessionLifecycle::Cancelled,
    ] {
        for disposed in [false, true] {
            assert_error(
                store(DirectStoreErrorV1::State(
                    StoreErrorV1::SessionStartStateConflictV1 {
                        lifecycle,
                        current_terminal_disposition: disposed,
                    },
                )),
                "session.start_replace",
                "SESSION_START_STATE_CONFLICT",
            );
        }
    }
    let value = assert_error(
        graph(GraphMutationErrorV2::ItemRevisionConflict {
            expected: Revision::new(2),
            actual: Revision::new(3),
        }),
        "item.set",
        "ITEM_REVISION_CONFLICT",
    );
    assert_eq!(value["details"]["expected_revision"], 2);
    assert_eq!(value["details"]["current_revision"], 3);
    let value = assert_error(
        DirectExecutionErrorV1::Domain(DomainError::SessionIdentityMismatch {
            expected: SessionId::new(ID).unwrap(),
            actual: Some(SessionId::new(OTHER).unwrap()),
        }),
        "session.begin",
        "SESSION_ID_MISMATCH",
    );
    assert_eq!(value["details"]["actual_session_id"], OTHER);
}

// READS-004: the typed fence-preflight facts facade must equal the Standard
// status read it replaces — identical failures with identical envelopes on
// every reachable condition, and identical facts on success.

mod reads004_equivalence {
    use podway_core::{BlockerState, SessionLifecycle, UnixMillis};
    use podway_engine::{
        direct_api::{
            DirectCommandV1, DirectRequestV1, DirectResponseV1, execute_direct_preflight_facts_v1,
            execute_direct_v1,
        },
        direct_execution::{
            DirectPreflightFactsResponseV1, DirectReadCommandV1, NativeDirectIdSourceV1,
            envelope_equivalent_status_bytes_v1,
        },
        execution::ExecutionClockV1,
        v2_read_service::GraphStatusTierV2,
        workspace::DirectWorkspaceResolverV1,
    };
    use podway_git::{
        DiagnosticPathDisplayV1, GitResolveErrorV1, GitResolverContractV1, LosslessPathV1,
        NativeGitResolverV1, ValidatedWorktreeV1, WorktreeRootsV1, WorktreeSelectorV1,
    };
    use podway_protocol::{Rfc3339MillisV1, SessionBlockV1, validate_daemon_free_document_v1};
    use podway_store::IdempotencyKeyV1;
    use serde_json::Value;

    use super::super::int_v2dfc006_direct_execution::{Clock, deadline, digest};
    use super::super::int_v2dfc006_races::{
        READS004_RESPONSE, cursor_fences, graph, prepare_fixture, reads004_facts_request,
    };

    fn resolver() -> DirectWorkspaceResolverV1<NativeGitResolverV1> {
        DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest())
    }

    fn status_request(selector: &WorktreeSelectorV1) -> DirectRequestV1 {
        DirectRequestV1 {
            selector: selector.clone(),
            expected_workspace_id: None,
            request_id: podway_protocol::RequestIdV1::new(READS004_RESPONSE).unwrap(),
            command: DirectCommandV1::Read {
                command: DirectReadCommandV1::Status {
                    tier: GraphStatusTierV2::Standard,
                    history_before: None,
                },
                expected_session_id: None,
            },
            idempotency_key: None,
            deadline: deadline(),
            observed_at: Rfc3339MillisV1::new("2023-11-14T22:13:20.001Z").unwrap(),
            timeout_ms: 5000,
        }
    }

    /// Both paths share one request identity and observation, so identical
    /// failures must produce byte-identical public envelopes.
    fn identical_failures(
        selector: &WorktreeSelectorV1,
        clock: &impl ExecutionClockV1,
    ) -> (Value, Value) {
        let old = match execute_direct_v1(
            &resolver(),
            status_request(selector),
            clock,
            &NativeDirectIdSourceV1,
        )
        .unwrap()
        {
            DirectResponseV1::Success(output) => {
                panic!("status unexpectedly succeeded: {}", output.as_value())
            }
            DirectResponseV1::Failure(error) => error.into_value(),
        };
        let new = match execute_direct_preflight_facts_v1(
            &resolver(),
            reads004_facts_request(selector),
            clock,
        )
        .unwrap()
        {
            DirectPreflightFactsResponseV1::Facts(facts) => {
                panic!("facts unexpectedly succeeded: {facts:?}")
            }
            DirectPreflightFactsResponseV1::Failure(error) => error.into_value(),
        };
        assert!(
            validate_daemon_free_document_v1("podway.error/v2", &old),
            "old path produced an invalid error envelope"
        );
        assert_eq!(
            serde_json::to_vec(&old).unwrap(),
            serde_json::to_vec(&new).unwrap(),
            "facts failure diverged from the Standard status failure"
        );
        (old, new)
    }

    struct YearTenThousand;
    impl ExecutionClockV1 for YearTenThousand {
        fn now(&self) -> UnixMillis {
            UnixMillis::new(253_402_300_800_000)
        }
    }

    #[test]
    fn reads004_facts_equal_the_standard_status_success_and_envelope_bytes() {
        let (temporary, selector, store) = prepare_fixture();
        let value = match execute_direct_v1(
            &resolver(),
            status_request(&selector),
            &Clock,
            &NativeDirectIdSourceV1,
        )
        .unwrap()
        {
            DirectResponseV1::Success(output) => output.into_value(),
            DirectResponseV1::Failure(error) => panic!("status failed: {}", error.into_value()),
        };
        let facts = match execute_direct_preflight_facts_v1(
            &resolver(),
            reads004_facts_request(&selector),
            &Clock,
        )
        .unwrap()
        {
            DirectPreflightFactsResponseV1::Facts(facts) => facts,
            DirectPreflightFactsResponseV1::Failure(error) => {
                panic!("facts failed: {}", error.into_value())
            }
        };
        // The typed facts equal what the old CLI parsed from the envelope.
        assert_eq!(
            facts.workspace_id.as_str(),
            value["workspace"]["uuid"].as_str().unwrap()
        );
        assert_eq!(
            facts.session_id.as_str(),
            value["result"]["session"]["id"].as_str().unwrap()
        );
        assert_eq!(
            facts.session_revision.get(),
            value["result"]["session"]["revision"].as_u64().unwrap()
        );
        assert_eq!(facts.lifecycle, SessionLifecycle::Running);
        assert_eq!(
            value["result"]["session"]["lifecycle"].as_str(),
            Some("running")
        );
        assert_eq!(
            facts
                .attempt_id
                .as_ref()
                .map(|attempt| attempt.as_str().to_owned()),
            value["result"]["current"]["attempt"]["attempt_id"]
                .as_str()
                .map(str::to_owned)
        );
        let parsed_items: Vec<(String, u64)> = value["result"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| {
                (
                    item["item_id"].as_str().unwrap().to_owned(),
                    item["revision"].as_u64().unwrap(),
                )
            })
            .collect();
        let facts_items: Vec<(String, u64)> = facts
            .item_revisions
            .iter()
            .map(|(id, revision)| (id.clone(), revision.get()))
            .collect();
        assert_eq!(facts_items, parsed_items);
        assert_eq!(
            facts.goal_revision.map(|revision| revision.get()),
            value["result"].get("goal_revision").and_then(Value::as_u64),
        );
        // C1 constructor parity on the real projected result: the exact
        // envelope-equivalent byte accounting equals the serialized length of
        // the envelope the old path actually built.
        assert_eq!(
            serde_json::to_vec(&value).unwrap().len(),
            envelope_equivalent_status_bytes_v1(
                &value["result"],
                value["workspace"]["root"].as_str().unwrap(),
                READS004_RESPONSE,
                value["workspace"]["uuid"].as_str().unwrap(),
                value["generated_at"].as_str().unwrap(),
                value["workspace"]["latest_workspace_sequence"]
                    .as_u64()
                    .unwrap(),
            )
        );
        drop(store);
        drop(temporary);
    }

    #[test]
    fn reads004_facts_fail_identically_on_persisted_timestamp_corruption() {
        let (temporary, selector, store) = prepare_fixture();
        let block = DirectCommandV1::Cursor(
            podway_engine::direct_execution::DirectCursorCommandV1::Block(SessionBlockV1 {
                reason: "reads004 equivalence blocker".into(),
                preconditions: cursor_fences(&graph(&store)),
            }),
        );
        let request = DirectRequestV1 {
            command: block,
            idempotency_key: Some(IdempotencyKeyV1::new("reads004-block").unwrap()),
            ..status_request(&selector)
        };
        match execute_direct_v1(&resolver(), request, &Clock, &NativeDirectIdSourceV1).unwrap() {
            DirectResponseV1::Success(_) => {}
            DirectResponseV1::Failure(error) => panic!("block failed: {}", error.into_value()),
        }
        let state = graph(&store);
        let attempt = state.trace().active_attempt().unwrap().attempt_id();
        let memory = state
            .workflow_memory()
            .attempts()
            .iter()
            .find(|memory| memory.attempt_id() == attempt)
            .unwrap();
        let blocker = memory
            .blockers()
            .iter()
            .find(|blocker| blocker.state() == BlockerState::Open)
            .expect("engine block created an open blocker");
        let connection = rusqlite::Connection::open(store.layout().database()).unwrap();
        connection
            .execute(
                "UPDATE v2_blockers SET created_at_ms = ?1 WHERE blocker_id = ?2",
                rusqlite::params![253_402_300_800_000i64, blocker.blocker_id().as_str()],
            )
            .unwrap();
        drop(connection);
        let (old, _) = identical_failures(&selector, &Clock);
        assert_eq!(old["code"], "INTERNAL_ERROR");
        // Multi-fault priority: the persisted projection failure keeps
        // precedence even when the clock is also out of range.
        let (multi, _) = identical_failures(&selector, &YearTenThousand);
        assert_eq!(multi["code"], old["code"]);
        drop(store);
        drop(temporary);
    }

    #[test]
    fn reads004_facts_fail_identically_on_an_out_of_range_clock() {
        let (temporary, selector, _store) = prepare_fixture();
        let (old, _) = identical_failures(&selector, &YearTenThousand);
        assert_eq!(old["code"], "REQUEST_INVALID");
        assert_eq!(old["exit_code"], 2);
        drop(temporary);
    }

    #[test]
    fn reads004_facts_fail_identically_on_corrupt_store_admission() {
        // The paired Store-admission corruption case: the current attempt's
        // recorded item value is replaced by an undecodable payload (READS-
        // 001's owned corruption pattern), so both facades refuse during the
        // eager Store decode — before projection — with identical public
        // envelopes and no read side effects.
        let (temporary, selector, store) = prepare_fixture();
        let attempt = graph(&store)
            .trace()
            .active_attempt()
            .unwrap()
            .attempt_id()
            .clone();
        {
            let connection = rusqlite::Connection::open(store.layout().database()).unwrap();
            connection
                .execute(
                    "UPDATE v2_item_slots SET value_json='{\"kind\":\"mystery\"}' \
                     WHERE attempt_id = ?1 AND item_id = 'i01'",
                    rusqlite::params![attempt.as_str()],
                )
                .unwrap();
        }
        // Read-side-effect preimage captured after the corruption write.
        let database_before = std::fs::read(store.layout().database()).unwrap();
        let wal_before = std::fs::read(store.layout().wal()).ok();
        let shm_before = store.layout().shm().exists();
        let (old, new) = identical_failures(&selector, &Clock);
        assert_eq!(old["code"], new["code"]);
        assert_eq!(old["code"], "WORKSPACE_STATE_UNREADABLE");
        assert_eq!(old["exit_code"], 5);
        assert_eq!(old["retryable"], false);
        // Both reads are write-free: identical files, sidecars and outcomes.
        assert_eq!(
            std::fs::read(store.layout().database()).unwrap(),
            database_before
        );
        assert_eq!(std::fs::read(store.layout().wal()).ok(), wal_before);
        assert_eq!(store.layout().shm().exists(), shm_before);
        drop(store);
        drop(temporary);
    }

    #[test]
    fn reads004_facts_fail_identically_on_a_wrong_expected_session() {
        let (temporary, selector, _store) = prepare_fixture();
        let wrong = podway_core::SessionId::new("00000000-0000-4000-8000-000000000099").unwrap();
        let mut old_request = status_request(&selector);
        old_request.command = DirectCommandV1::Read {
            command: DirectReadCommandV1::Status {
                tier: GraphStatusTierV2::Standard,
                history_before: None,
            },
            expected_session_id: Some(wrong.clone()),
        };
        let old = match execute_direct_v1(&resolver(), old_request, &Clock, &NativeDirectIdSourceV1)
            .unwrap()
        {
            DirectResponseV1::Success(output) => {
                panic!("status unexpectedly succeeded: {}", output.as_value())
            }
            DirectResponseV1::Failure(error) => error.into_value(),
        };
        let facts_request = podway_engine::direct_execution::DirectPreflightFactsRequestV1 {
            expected_session_id: Some(wrong),
            ..reads004_facts_request(&selector)
        };
        let new =
            match execute_direct_preflight_facts_v1(&resolver(), facts_request, &Clock).unwrap() {
                DirectPreflightFactsResponseV1::Facts(facts) => {
                    panic!("facts unexpectedly succeeded: {facts:?}")
                }
                DirectPreflightFactsResponseV1::Failure(error) => error.into_value(),
            };
        assert_eq!(old["code"], "SESSION_ID_MISMATCH");
        assert_eq!(
            serde_json::to_vec(&old).unwrap(),
            serde_json::to_vec(&new).unwrap(),
            "facts diverged on the expected-session fence"
        );
        drop(temporary);
    }

    /// Rebuilding a worktree through the public `ValidatedWorktreeV1::new`
    /// supplies `TransientWorktreeEvidenceV1::Unavailable`; workspace
    /// revalidation requires native transient Git-object evidence on both
    /// sides, so a rebuilt worktree is refused before either facade reaches
    /// Stage 5 — independent of the display text. `display: None`
    /// reconstructs the unchanged native display as the control proving that
    /// boundary.
    struct RebuiltWorktreeResolver {
        display: Option<String>,
        inner: NativeGitResolverV1,
    }
    impl GitResolverContractV1 for RebuiltWorktreeResolver {
        fn resolve(
            &self,
            selector: WorktreeSelectorV1,
        ) -> Result<ValidatedWorktreeV1, GitResolveErrorV1> {
            let native = self.inner.resolve(selector)?;
            let roots = native.roots();
            let display = DiagnosticPathDisplayV1::new(
                self.display
                    .clone()
                    .unwrap_or_else(|| roots.worktree_root().display().as_str().to_owned()),
            )
            .unwrap();
            let rebuilt = WorktreeRootsV1::new(
                LosslessPathV1::from_base64url(
                    roots.worktree_root().path_bytes_base64url().clone(),
                    display,
                ),
                roots.common_directory_root().clone(),
                roots.worktree_administration_root().clone(),
            );
            Ok(ValidatedWorktreeV1::new(
                native.identity().clone(),
                rebuilt,
                native.kind().clone(),
                native.containment().clone(),
                native.move_metadata().clone(),
            )
            .unwrap())
        }
    }

    #[test]
    fn reads004_facts_reject_rebuilt_worktrees_identically_at_the_evidence_boundary() {
        // Public-constructor worktrees lack the native transient Git-object
        // evidence revalidation requires, so they are refused before either
        // facade reaches its Stage 5 workspace checks. The unchanged-display
        // control proves the refusal is that evidence boundary and not the
        // display text; the divergent displays fail byte-identically to it.
        // This establishes no facade-level Stage 5 root reachability or
        // display-coherence claim; root-condition coverage stays with the
        // retained Stage 5 predicate and the C1 component checks.
        let scenarios: &[(&str, Option<String>)] = &[
            ("unchanged-native-display control", None),
            ("relative display", Some("fixture".to_owned())),
            ("oversized display", Some(format!("/{}", "a".repeat(4_096)))),
        ];
        let mut control: Option<Value> = None;
        for (name, display) in scenarios {
            let (temporary, selector, _store) = prepare_fixture();
            let resolver = DirectWorkspaceResolverV1::new(
                RebuiltWorktreeResolver {
                    display: display.clone(),
                    inner: NativeGitResolverV1::new(),
                },
                digest(),
            );
            let old = match execute_direct_v1(
                &resolver,
                status_request(&selector),
                &Clock,
                &NativeDirectIdSourceV1,
            )
            .unwrap()
            {
                DirectResponseV1::Success(output) => {
                    panic!("status unexpectedly succeeded: {}", output.as_value())
                }
                DirectResponseV1::Failure(error) => error.into_value(),
            };
            let new = match execute_direct_preflight_facts_v1(
                &resolver,
                reads004_facts_request(&selector),
                &Clock,
            )
            .unwrap()
            {
                DirectPreflightFactsResponseV1::Facts(facts) => {
                    panic!("facts unexpectedly succeeded: {name}: {facts:?}")
                }
                DirectPreflightFactsResponseV1::Failure(error) => error.into_value(),
            };
            assert_eq!(
                old["code"], "WORKSPACE_STATE_UNREADABLE",
                "{name} changed the original admission boundary"
            );
            assert_eq!(
                serde_json::to_vec(&old).unwrap(),
                serde_json::to_vec(&new).unwrap(),
                "facts diverged from the status read at {name}"
            );
            if let Some(control) = &control {
                assert_eq!(
                    serde_json::to_vec(control).unwrap(),
                    serde_json::to_vec(&old).unwrap(),
                    "{name} diverged from the unchanged-display control"
                );
            } else {
                control = Some(old);
            }
            drop(temporary);
        }
    }
}
