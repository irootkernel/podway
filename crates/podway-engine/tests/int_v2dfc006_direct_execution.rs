//! Real Git and current-generation Store boundaries without an installed runtime.

use std::{
    os::unix::ffi::OsStrExt,
    process::Command,
    time::{Duration, Instant},
};

use podway_core::{Sha256Digest, UnixMillis, WorkspaceId, canonicalize_json_v1};
use podway_engine::{
    direct_execution::{DirectExecutionErrorV1, OutcomeLookupRequestV1, lookup_outcome_v1},
    execution::ExecutionClockV1,
    workspace::{
        DirectWorkspaceResolutionErrorV1, DirectWorkspaceResolverV1, WorkspaceResolutionErrorV1,
    },
};
use podway_git::{
    DiagnosticPathDisplayV1, GitResolverContractV1, LosslessPathV1, NativeGitResolverV1,
    WORKTREE_SELECTOR_VERSION_V1, WorkspaceLayoutInitializerV1, WorktreeSelectorV1,
};
use podway_protocol::{OutputEnvelopeV4, RequestIdV1, validate_daemon_free_document_v1};
use podway_store::{
    CanonicalExecutionJsonV1, IdempotencyKeyV1, ValidatedWorkspaceRootV1,
    direct_store::{
        DirectMutationRequestV1, DirectOutcomeScopeV1, DirectStoreV1, DirectWorkspaceLayoutV1,
        ExclusiveMaintenanceLeaseV1,
    },
};
use serde_json::json;
use tempfile::TempDir;

const WORKSPACE: &str = "00000000-0000-4000-8000-000000000001";
const ORIGINAL_RESPONSE: &str = "00000000-0000-4000-8000-000000000002";
const LOOKUP_RESPONSE: &str = "00000000-0000-4000-8000-000000000003";
const NOW: u64 = 1_700_000_000_000;
const SIMPLE: &[u8] = br#"{"schema":"podway.procedure/v2","id":"direct-views","version":"1",
    "name":"Direct views","purpose":"Exercise direct read families.",
    "node_definitions":{"work":{"type":"action","title":"Work","intent":"Record work.",
        "items":[{"id":"note","type":"text","prompt":"Record a note.","required":false,"max_length":1000}]}},
    "graph":{"entry":"work","nodes":[{"id":"work","use":"work","terminal":true}]}}"#;

const ITEMS: &[u8] = br#"{"schema":"podway.procedure/v2","id":"direct-items","version":"1",
    "name":"Direct items","purpose":"Exercise actual direct item mutations.",
    "node_definitions":{"work":{"type":"action","title":"Work","intent":"Record work.",
        "items":[{"id":"checked","type":"confirm","prompt":"Confirm.","required":false},
        {"id":"note","type":"text","prompt":"Record a note.","required":false},
        {"id":"list","type":"list","prompt":"Record a list.","required":false},
        {"id":"report","type":"artifact","prompt":"Attach report.","required":false}]}},
    "graph":{"entry":"work","nodes":[{"id":"work","use":"work","terminal":true}]}}"#;

fn delegated_procedure() -> Vec<u8> {
    let mut procedure: serde_json::Value = serde_json::from_slice(ITEMS).unwrap();
    procedure["node_definitions"]["work"]["items"][0]["required"] = json!(true);
    procedure["graph"]["nodes"][0]["delegation"] = json!({"binding_ref":"local-specialist", "role_ref":"reviewer",
        "role_snapshot_digest":digest(), "max_rounds":3, "after_accept":"retain"});
    serde_json::to_vec(&procedure).unwrap()
}

#[test]
fn direct_start_confirms_source_and_replays_without_source_clock_or_identifiers() {
    use podway_core::{
        AttemptId, BlockerId, DelegationReceiptIdV1, ProcedureSnapshotId, SessionId,
    };
    use podway_engine::{
        direct_execution::{DirectIdSourceV1, NativeDirectIdSourceV1},
        direct_start::{DirectStartCommandV1, DirectStartRequestV1, start_session_v1},
        execution::{ProcedureV2StartPreparationErrorV1, prepare_workspace_procedure_source_v2},
    };
    use podway_protocol::{SessionStartSourceV1, SessionStartV1};
    use podway_store::direct_store::DirectStoreErrorV1;
    struct NoEffects;
    impl ExecutionClockV1 for NoEffects {
        fn now(&self) -> UnixMillis {
            panic!("rejection/replay read the clock")
        }
    }
    impl DirectIdSourceV1 for NoEffects {
        fn next_workspace_id(&self) -> WorkspaceId {
            panic!("unexpected workspace ID")
        }
        fn next_session_id(&self) -> SessionId {
            panic!("rejection/replay generated a session")
        }
        fn next_attempt_id(&self) -> AttemptId {
            panic!("unexpected attempt")
        }
        fn next_blocker_id(&self) -> BlockerId {
            panic!("unexpected blocker")
        }
        fn next_procedure_snapshot_id(&self) -> ProcedureSnapshotId {
            panic!("rejection/replay generated a snapshot")
        }
        fn next_delegation_receipt_id(&self) -> DelegationReceiptIdV1 {
            panic!("unexpected receipt")
        }
    }
    let (fixture, selector, store, _) = fixture();
    let path = fixture.path().join("start.json");
    std::fs::write(&path, SIMPLE).unwrap();
    let resolver = DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest());
    let admitted = prepare_workspace_procedure_source_v2("start.json", SIMPLE).unwrap();
    let request = |key: Option<&str>, dry_run, guard| DirectStartRequestV1 {
        selector: selector.clone(),
        expected_workspace_id: None,
        request_id: RequestIdV1::new(ORIGINAL_RESPONSE).unwrap(),
        key: key.map(|key| IdempotencyKeyV1::new(key).unwrap()),
        command: DirectStartCommandV1::Start(SessionStartV1 {
            source: SessionStartSourceV1::Procedure {
                procedure: "start.json".into(),
            },
            expected_procedure_digest: guard,
            task_title: "Actual start".into(),
            dry_run,
        }),
        deadline: deadline(),
    };
    let preview =
        start_session_v1(&resolver, request(None, true, None), &Clock, &NoEffects).unwrap();
    assert_eq!(preview.as_value()["result"]["dry_run"], true);
    assert!(preview.as_value().get("session").is_none());
    assert_eq!(
        store
            .read(deadline(), |snapshot| snapshot.committed_sequence())
            .unwrap(),
        1
    );
    assert!(matches!(
        start_session_v1(
            &resolver,
            request(Some("no-confirmation"), false, None),
            &NoEffects,
            &NoEffects
        ),
        Err(DirectExecutionErrorV1::Start(
            ProcedureV2StartPreparationErrorV1::DigestConfirmationRequired { .. }
        ))
    ));
    let original_request = request(Some("start"), false, Some(admitted.digest().clone()));
    let original = start_session_v1(
        &resolver,
        original_request.clone(),
        &Clock,
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    assert_eq!(original.as_value()["session"]["lifecycle"], "prepared");
    assert_eq!(original.as_value()["session"]["revision_after"], 0);
    assert_eq!(
        original.as_value()["workspace"]["latest_workspace_sequence"],
        2
    );
    let mut changed: serde_json::Value = serde_json::from_slice(SIMPLE).unwrap();
    changed["name"] = json!("Changed after committed start");
    std::fs::write(&path, serde_json::to_vec(&changed).unwrap()).unwrap();
    assert_eq!(
        start_session_v1(&resolver, original_request.clone(), &NoEffects, &NoEffects).unwrap(),
        original
    );
    let mut changed_request = original_request.clone();
    let DirectStartCommandV1::Start(input) = &mut changed_request.command else {
        unreachable!()
    };
    input.task_title = "Different semantic request".into();
    assert!(matches!(
        start_session_v1(&resolver, changed_request, &NoEffects, &NoEffects),
        Err(DirectExecutionErrorV1::Store(
            DirectStoreErrorV1::IdempotencyKeyReused
        ))
    ));
    std::fs::remove_file(&path).unwrap();
    assert_eq!(
        start_session_v1(&resolver, original_request, &NoEffects, &NoEffects).unwrap(),
        original
    );
    assert!(matches!(
        start_session_v1(
            &resolver,
            request(Some("missing"), false, Some(admitted.digest().clone())),
            &NoEffects,
            &NoEffects
        ),
        Err(DirectExecutionErrorV1::Start(
            ProcedureV2StartPreparationErrorV1::Source(_)
        ))
    ));
    store
        .read(deadline(), |snapshot| {
            assert!(
                snapshot
                    .outcome(&IdempotencyKeyV1::new("no-confirmation").unwrap())?
                    .is_none()
            );
            assert!(
                snapshot
                    .outcome(&IdempotencyKeyV1::new("missing").unwrap())?
                    .is_none()
            );
            assert_eq!(snapshot.committed_sequence()?, 2);
            assert_eq!(
                snapshot.current_graph()?.unwrap().trace().revision(),
                podway_core::Revision::ZERO
            );
            Ok(())
        })
        .unwrap();
}

#[test]
fn typed_direct_api_returns_validated_success_and_rejection_without_effects() {
    use podway_core::{AttemptId, Revision, SessionId};
    use podway_engine::{
        direct_api::{DirectCommandV1, DirectRequestV1, DirectResponseV1, execute_direct_v1},
        direct_execution::NativeDirectIdSourceV1,
        direct_items::DirectItemCommandV1,
        direct_start::DirectStartCommandV1,
        execution::prepare_workspace_procedure_source_v2,
    };
    use podway_protocol::{
        ItemAttachSourceV1, ItemAttachV1, ItemMutationPreconditionsWireV1, ItemSetV1,
        ProcedureV2MutationCommandV1, Rfc3339MillisV1, SessionBeginV1,
        SessionIdentityPreconditionsWireV1, SessionStartSourceV1, SessionStartV1,
    };
    struct NoClock;
    impl ExecutionClockV1 for NoClock {
        fn now(&self) -> UnixMillis {
            panic!("rejected/replayed mutation stamped time")
        }
    }
    let (fixture, selector, store, _) = fixture();
    std::fs::write(fixture.path().join("api.json"), ITEMS).unwrap();
    let resolver = DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest());
    let request = |command, key: Option<&str>| DirectRequestV1 {
        command,
        selector: selector.clone(),
        expected_workspace_id: Some(WorkspaceId::new(WORKSPACE).unwrap()),
        request_id: RequestIdV1::new(ORIGINAL_RESPONSE).unwrap(),
        idempotency_key: key.map(|key| IdempotencyKeyV1::new(key).unwrap()),
        deadline: deadline(),
        observed_at: Rfc3339MillisV1::from_unix_millis(NOW).unwrap(),
        timeout_ms: 5000,
    };
    let success = |response| match response {
        DirectResponseV1::Success(output) => output,
        DirectResponseV1::Failure(error) => {
            panic!("unexpected rejection: {:?}", error.into_value())
        }
    };
    let rejection = |response, code| match response {
        DirectResponseV1::Failure(error) => {
            let value = error.into_value();
            assert_eq!(value["code"], code);
            assert!(validate_daemon_free_document_v1("podway.error/v2", &value));
            value
        }
        DirectResponseV1::Success(output) => panic!("unexpected success: {:?}", output.as_value()),
    };
    let input = SessionStartV1 {
        source: SessionStartSourceV1::Procedure {
            procedure: "api.json".into(),
        },
        expected_procedure_digest: Some(
            prepare_workspace_procedure_source_v2("api.json", ITEMS)
                .unwrap()
                .digest()
                .clone(),
        ),
        task_title: "Unified API".into(),
        dry_run: false,
    };
    let start = DirectCommandV1::Start(DirectStartCommandV1::Start(input));
    rejection(
        execute_direct_v1(
            &resolver,
            request(start.clone(), None),
            &NoClock,
            &NativeDirectIdSourceV1,
        )
        .unwrap(),
        "REQUEST_INVALID",
    );
    let original = success(
        execute_direct_v1(
            &resolver,
            request(start.clone(), Some("api-start")),
            &Clock,
            &NativeDirectIdSourceV1,
        )
        .unwrap(),
    );
    let replay = success(
        execute_direct_v1(
            &resolver,
            request(start, Some("api-start")),
            &NoClock,
            &NativeDirectIdSourceV1,
        )
        .unwrap(),
    );
    assert_eq!(original.as_value(), replay.as_value());
    let session = SessionId::new(original.as_value()["session"]["id"].as_str().unwrap()).unwrap();
    let begin =
        DirectCommandV1::Procedure(ProcedureV2MutationCommandV1::SessionBegin(SessionBeginV1 {
            initial_goal: None,
            preconditions: SessionIdentityPreconditionsWireV1 {
                expected_session_id: session.clone(),
                expected_session_revision: Revision::ZERO,
            },
        }));
    success(
        execute_direct_v1(
            &resolver,
            request(begin, Some("api-begin")),
            &Clock,
            &NativeDirectIdSourceV1,
        )
        .unwrap(),
    );
    let graph = store.prepare_graph(deadline()).unwrap().unwrap();
    let attempt: AttemptId = graph
        .graph()
        .trace()
        .active_attempt()
        .unwrap()
        .attempt_id()
        .clone();
    let set = |revision| {
        DirectCommandV1::Item(DirectItemCommandV1::Set(ItemSetV1 {
            item_id: podway_core::ItemId::new("note").unwrap(),
            value: "written through API".into(),
            preconditions: ItemMutationPreconditionsWireV1 {
                expected_session_id: session.clone(),
                expected_attempt_id: attempt.clone(),
                expected_item_revision: Revision::new(revision),
            },
        }))
    };
    let before = store
        .read(deadline(), |snapshot| snapshot.committed_sequence())
        .unwrap();
    for (path, key, code) in [
        (
            "missing-report.txt",
            "api-missing-artifact",
            "ARTIFACT_NOT_FOUND",
        ),
        (
            "../outside.txt",
            "api-outside-artifact",
            "PATH_OUTSIDE_WORKTREE",
        ),
    ] {
        let attach = DirectCommandV1::Item(DirectItemCommandV1::Attach(ItemAttachV1 {
            item_id: podway_core::ItemId::new("report").unwrap(),
            source: ItemAttachSourceV1::Path {
                path: path.into(),
                media_type: None,
            },
            preconditions: ItemMutationPreconditionsWireV1 {
                expected_session_id: session.clone(),
                expected_attempt_id: attempt.clone(),
                expected_item_revision: Revision::ZERO,
            },
        }));
        rejection(
            execute_direct_v1(
                &resolver,
                request(attach, Some(key)),
                &NoClock,
                &NativeDirectIdSourceV1,
            )
            .unwrap(),
            code,
        );
        assert!(
            store
                .read(deadline(), |snapshot| snapshot
                    .outcome(&IdempotencyKeyV1::new(key).unwrap()))
                .unwrap()
                .is_none()
        );
    }

    rejection(
        execute_direct_v1(
            &resolver,
            request(set(99), Some("api-stale")),
            &NoClock,
            &NativeDirectIdSourceV1,
        )
        .unwrap(),
        "ITEM_REVISION_CONFLICT",
    );
    assert_eq!(
        store
            .read(deadline(), |snapshot| snapshot.committed_sequence())
            .unwrap(),
        before
    );
    assert!(
        store
            .read(deadline(), |snapshot| snapshot
                .outcome(&IdempotencyKeyV1::new("api-stale").unwrap()))
            .unwrap()
            .is_none()
    );
    let output = success(
        execute_direct_v1(
            &resolver,
            request(set(0), Some("api-set")),
            &Clock,
            &NativeDirectIdSourceV1,
        )
        .unwrap(),
    );
    assert!(validate_daemon_free_document_v1(
        "podway.output/v4",
        output.as_value()
    ));
    assert_eq!(
        output.as_value()["workspace"]["latest_workspace_sequence"],
        before + 1
    );
}

#[test]
fn marked_reads_return_the_original_maintenance_key_without_resuming_or_writing() {
    use podway_engine::{
        direct_api::{DirectCommandV1, DirectRequestV1, DirectResponseV1, execute_direct_v1},
        direct_execution::{DirectReadCommandV1, NativeDirectIdSourceV1},
        direct_maintenance::{
            DirectMaintenanceBoundaryV1, DirectMaintenanceRequestV1,
            remove_workspace_with_boundary_observer_v1, reset_all_with_boundary_observer_v1,
        },
    };
    use podway_protocol::Rfc3339MillisV1;
    struct NoClock;
    impl ExecutionClockV1 for NoClock {
        fn now(&self) -> UnixMillis {
            panic!("marked read resumed maintenance")
        }
    }
    for (remove, command, boundary) in [
        (false, "workspace.reset_all", "reset_marker"),
        (true, "workspace.remove", "removal_marker"),
    ] {
        let (_fixture, selector, store, _) = fixture();
        let resolver = DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest());
        let request = DirectMaintenanceRequestV1 {
            selector: selector.clone(),
            expected_workspace_id: Some(WorkspaceId::new(WORKSPACE).unwrap()),
            request_id: RequestIdV1::new(ORIGINAL_RESPONSE).unwrap(),
            key: IdempotencyKeyV1::new("original-maintenance-key").unwrap(),
            confirmed: true,
            force: remove,
            deadline: deadline(),
        };
        let observer = |point| {
            if point == DirectMaintenanceBoundaryV1::MarkerPublished {
                panic!("isolate a published marker");
            }
        };
        let interrupted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if remove {
                remove_workspace_with_boundary_observer_v1(&resolver, request, &Clock, observer)
                    .unwrap();
            } else {
                reset_all_with_boundary_observer_v1(
                    &resolver,
                    request,
                    &Clock,
                    &NativeDirectIdSourceV1,
                    observer,
                )
                .unwrap();
            }
        }));
        assert!(interrupted.is_err());
        let bytes = || {
            let mut entries = std::fs::read_dir(store.layout().runtime_directory())
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .filter(|path| path.is_file())
                .collect::<Vec<_>>();
            entries.sort();
            entries
                .into_iter()
                .map(|path| {
                    let bytes = std::fs::read(&path).unwrap();
                    (path, bytes)
                })
                .collect::<Vec<_>>()
        };
        let before = bytes();
        let request = DirectRequestV1 {
            selector,
            expected_workspace_id: None,
            request_id: RequestIdV1::new(LOOKUP_RESPONSE).unwrap(),
            idempotency_key: None,
            command: DirectCommandV1::Read {
                command: DirectReadCommandV1::Observe,
                expected_session_id: None,
            },
            observed_at: Rfc3339MillisV1::from_unix_millis(NOW).unwrap(),
            timeout_ms: 5000,
            deadline: deadline(),
        };
        let DirectResponseV1::Failure(error) =
            execute_direct_v1(&resolver, request, &NoClock, &NativeDirectIdSourceV1).unwrap()
        else {
            panic!("a marked read must fail closed")
        };
        let value = error.into_value();
        assert_eq!(value["code"], "WORKSPACE_MAINTENANCE");
        assert_eq!(value["details"]["boundary"], boundary);
        assert_eq!(value["details"]["recovery"]["command"], command);
        assert_eq!(
            value["details"]["recovery"]["argv"]
                .as_array()
                .unwrap()
                .last()
                .unwrap(),
            "original-maintenance-key"
        );
        assert_eq!(
            value["details"]["recovery"]["requires_explicit_authorization"],
            true
        );
        assert!(validate_daemon_free_document_v1("podway.error/v2", &value));
        assert_eq!(bytes(), before);
    }
}

#[test]
fn direct_start_replacement_preserves_terminal_and_superseded_history_atomically() {
    use podway_core::{Revision, SessionLifecycle};
    use podway_engine::{
        direct_execution::{
            DirectBeginRequestV1, DirectCursorCommandV1, DirectCursorRequestV1,
            NativeDirectIdSourceV1, begin_session_v1, execute_cursor_v1,
        },
        direct_procedure::{DirectProcedureRequestV1, execute_procedure_v1},
        direct_start::{DirectStartCommandV1 as C, DirectStartRequestV1, start_session_v1},
        execution::prepare_workspace_procedure_source_v2,
    };
    use podway_protocol::{
        ProcedureV2MutationCommandV1, SessionBeginV1, SessionCancelV1,
        SessionDeletionModeV1 as Mode, SessionIdentityPreconditionsWireV1,
        SessionMutationPreconditionsWireV1, SessionStartReplaceV1, SessionStartSourceV1,
        SessionStartV1, SessionTerminalDispositionV1, TerminalDispositionInputV1,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct CountClock(AtomicUsize);
    impl ExecutionClockV1 for CountClock {
        fn now(&self) -> UnixMillis {
            UnixMillis::new(NOW + 20 + self.0.fetch_add(1, Ordering::SeqCst) as u64)
        }
    }
    for case in 0..6 {
        let (fixture, selector, store, _) = fixture();
        let source_path = fixture.path().join("start.json");
        std::fs::write(&source_path, SIMPLE).unwrap();
        let source = prepare_workspace_procedure_source_v2("start.json", SIMPLE).unwrap();
        let resolver = DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest());
        let clock = CountClock(AtomicUsize::new(0));
        let ids = NativeDirectIdSourceV1;
        let payload = || SessionStartV1 {
            source: SessionStartSourceV1::Procedure {
                procedure: "start.json".into(),
            },
            expected_procedure_digest: Some(source.digest().clone()),
            task_title: "Replacement policy".into(),
            dry_run: false,
        };
        let start_request = |key: &str, command| DirectStartRequestV1 {
            selector: selector.clone(),
            expected_workspace_id: None,
            request_id: RequestIdV1::new(ORIGINAL_RESPONSE).unwrap(),
            key: Some(IdempotencyKeyV1::new(key).unwrap()),
            command,
            deadline: deadline(),
        };
        start_session_v1(
            &resolver,
            start_request("original-start", C::Start(payload())),
            &clock,
            &ids,
        )
        .unwrap();
        let current = || {
            store
                .read(deadline(), |snapshot| snapshot.current_graph())
                .unwrap()
                .unwrap()
        };
        let identity_fences = || {
            let graph = current();
            SessionIdentityPreconditionsWireV1 {
                expected_session_id: graph.trace().session_id().clone(),
                expected_session_revision: graph.trace().revision(),
            }
        };
        if case > 0 {
            begin_session_v1(
                &resolver,
                DirectBeginRequestV1 {
                    selector: selector.clone(),
                    expected_workspace_id: None,
                    request_id: RequestIdV1::new(ORIGINAL_RESPONSE).unwrap(),
                    key: IdempotencyKeyV1::new("begin").unwrap(),
                    input: SessionBeginV1 {
                        initial_goal: None,
                        preconditions: identity_fences(),
                    },
                    deadline: deadline(),
                },
                &clock,
                &ids,
            )
            .unwrap();
        }
        if case >= 3 {
            let graph = current();
            execute_cursor_v1(
                &resolver,
                DirectCursorRequestV1 {
                    selector: selector.clone(),
                    expected_workspace_id: None,
                    request_id: RequestIdV1::new(ORIGINAL_RESPONSE).unwrap(),
                    key: IdempotencyKeyV1::new("cancel").unwrap(),
                    command: DirectCursorCommandV1::Cancel(SessionCancelV1 {
                        reason: "Finish previous work".into(),
                        preconditions: SessionMutationPreconditionsWireV1 {
                            expected_session_id: graph.trace().session_id().clone(),
                            expected_session_revision: graph.trace().revision(),
                            expected_attempt_id: graph
                                .trace()
                                .active_attempt()
                                .unwrap()
                                .attempt_id()
                                .clone(),
                        },
                    }),
                    deadline: deadline(),
                },
                &clock,
                &ids,
            )
            .unwrap();
        }
        if case >= 4 {
            execute_procedure_v1(
                &resolver,
                DirectProcedureRequestV1 {
                    selector: selector.clone(),
                    expected_workspace_id: None,
                    request_id: RequestIdV1::new(ORIGINAL_RESPONSE).unwrap(),
                    key: IdempotencyKeyV1::new("disposition").unwrap(),
                    command: ProcedureV2MutationCommandV1::SessionTerminalDisposition(
                        SessionTerminalDispositionV1 {
                            disposition: TerminalDispositionInputV1::NotRequired {
                                reason: "No external handoff".into(),
                            },
                            actor: None,
                            preconditions: identity_fences(),
                        },
                    ),
                    deadline: deadline(),
                },
                &clock,
                &ids,
            )
            .unwrap();
        }
        let old = current();
        let command = match case {
            4 => C::Start(payload()),
            _ => C::Replace(SessionStartReplaceV1 {
                start: payload(),
                preconditions: identity_fences(),
                mode: match case {
                    0 => Mode::Delete {
                        confirmed: true,
                        progress_summary: None,
                    },
                    1 => Mode::Delete {
                        confirmed: true,
                        progress_summary: Some("Recorded current progress".into()),
                    },
                    2 => Mode::Supersede {
                        reason: "Preserve prior work".into(),
                        actor: Some("coordinator".into()),
                    },
                    3 => Mode::DisposeAndArchive {
                        disposition: TerminalDispositionInputV1::HandedOff {
                            summary: "Previous result accepted".into(),
                            reference: "commit:prior".into(),
                        },
                        actor: Some("coordinator".into()),
                    },
                    5 => Mode::Eligible,
                    _ => unreachable!(),
                },
            }),
        };
        let mut preview_command = command.clone();
        match &mut preview_command {
            C::Start(input) => input.dry_run = true,
            C::Replace(input) => input.start.dry_run = true,
        }
        let before = store
            .read(deadline(), |snapshot| snapshot.committed_sequence())
            .unwrap();
        let preview = start_session_v1(
            &resolver,
            start_request("preview", preview_command),
            &clock,
            &ids,
        )
        .unwrap();
        assert_eq!(
            preview.as_value()["result"]["existing_session"]["action"],
            match case {
                0 | 1 => "delete",
                2 => "supersede_archive",
                3 => "dispose_archive",
                _ => "archive",
            }
        );
        assert_eq!(
            store
                .read(deadline(), |snapshot| snapshot.committed_sequence())
                .unwrap(),
            before
        );
        let original_request = start_request("replace", command);
        let replacement =
            start_session_v1(&resolver, original_request.clone(), &clock, &ids).unwrap();
        let calls = clock.0.load(Ordering::SeqCst);
        std::fs::remove_file(source_path).unwrap();
        assert_eq!(
            start_session_v1(&resolver, original_request, &clock, &ids).unwrap(),
            replacement
        );
        assert_eq!(clock.0.load(Ordering::SeqCst), calls);
        store
            .read(deadline(), |snapshot| {
                let next = snapshot.current_graph()?.unwrap();
                assert_ne!(next.trace().session_id(), old.trace().session_id());
                assert_eq!(next.trace().lifecycle(), SessionLifecycle::Prepared);
                assert_eq!(next.trace().revision(), Revision::ZERO);
                assert_eq!(snapshot.committed_sequence()?, before + 1);
                let archive = snapshot.archived_graph(old.trace().session_id())?;
                assert_eq!(archive.is_some(), case >= 2, "case {case}");
                assert_eq!(
                    snapshot
                        .outcome(&IdempotencyKeyV1::new("original-start").unwrap())?
                        .is_some(),
                    case >= 2
                );
                if case == 2 {
                    let view = snapshot
                        .archived_view(old.trace().session_id(), UnixMillis::new(NOW + 100))?
                        .unwrap();
                    assert_eq!(
                        view.graph().unwrap().trace().lifecycle(),
                        SessionLifecycle::Cancelled
                    );
                    assert_eq!(
                        view.terminal_dispositions()[0].successor_session_id(),
                        Some(next.trace().session_id())
                    );
                }
                Ok(())
            })
            .unwrap();
    }
}

#[test]
fn delegated_acceptance_recomputes_package_and_blocks_changed_items_before_clock() {
    use podway_core::{AttemptId, ItemId, ProcedureSnapshotId, Revision, SessionId};
    use podway_engine::{
        direct_delegation::{DirectDelegationRequestV1, accept_delegation_v1},
        direct_execution::{
            DirectCursorCommandV1, DirectCursorRequestV1, DirectDelegationFailureV1,
            DirectReadCommandV1, DirectReadRequestV1, NativeDirectIdSourceV1, execute_cursor_v1,
            execute_read_v1,
        },
        direct_items::{DirectItemCommandV1, DirectItemRequestV1, execute_item_v1},
        execution::workspace_procedure_snapshot_from_bytes_v2,
    };
    use podway_protocol::{
        DelegationAcceptV1, ItemCheckV1, ItemMutationPreconditionsWireV1, ItemSetV1,
        SessionCompleteV1, SessionMutationPreconditionsWireV1,
    };
    struct MutationClock;
    impl ExecutionClockV1 for MutationClock {
        fn now(&self) -> UnixMillis {
            UnixMillis::new(NOW + 20)
        }
    }
    struct NoClock;
    impl ExecutionClockV1 for NoClock {
        fn now(&self) -> UnixMillis {
            panic!("delegation rejection/replay read the mutation clock")
        }
    }
    let (_fixture, selector, store, _) = fixture();
    let source = delegated_procedure();
    let snapshot = workspace_procedure_snapshot_from_bytes_v2(
        "delegated.json",
        &source,
        ProcedureSnapshotId::new(LOOKUP_RESPONSE).unwrap(),
        UnixMillis::new(NOW + 2),
    )
    .unwrap();
    let graph = podway_store::GraphSessionStateV2::prepared(
        Revision::new(1),
        "Delegation fixture",
        snapshot,
        SessionId::new(LOOKUP_RESPONSE).unwrap(),
        UnixMillis::new(NOW + 2),
    )
    .unwrap()
    .begin_v2(
        Revision::ZERO,
        AttemptId::new(LOOKUP_RESPONSE).unwrap(),
        None,
        UnixMillis::new(NOW + 3),
    )
    .unwrap()
    .into_state();
    seed_graph(&store, &graph);
    let resolver = DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest());
    let current = || {
        store
            .read(deadline(), |snapshot| snapshot.current_graph())
            .unwrap()
            .unwrap()
    };
    let fences = || {
        let graph = current();
        SessionMutationPreconditionsWireV1 {
            expected_session_id: graph.trace().session_id().clone(),
            expected_session_revision: graph.trace().revision(),
            expected_attempt_id: graph.trace().active_attempt().unwrap().attempt_id().clone(),
        }
    };
    let complete = |key: &str| DirectCursorRequestV1 {
        selector: selector.clone(),
        expected_workspace_id: None,
        request_id: RequestIdV1::new(ORIGINAL_RESPONSE).unwrap(),
        key: IdempotencyKeyV1::new(key).unwrap(),
        command: DirectCursorCommandV1::Complete(SessionCompleteV1 {
            preconditions: fences(),
        }),
        deadline: deadline(),
    };
    assert!(matches!(
        execute_cursor_v1(
            &resolver,
            complete("missing-receipt"),
            &NoClock,
            &NativeDirectIdSourceV1
        ),
        Err(DirectExecutionErrorV1::Delegation(
            DirectDelegationFailureV1::ReceiptRequired
        ))
    ));
    let item = |key: &str, command| DirectItemRequestV1 {
        selector: selector.clone(),
        expected_workspace_id: None,
        request_id: RequestIdV1::new(ORIGINAL_RESPONSE).unwrap(),
        key: IdempotencyKeyV1::new(key).unwrap(),
        command,
        deadline: deadline(),
    };
    let item_fences = |revision| ItemMutationPreconditionsWireV1 {
        expected_session_id: SessionId::new(LOOKUP_RESPONSE).unwrap(),
        expected_attempt_id: AttemptId::new(LOOKUP_RESPONSE).unwrap(),
        expected_item_revision: Revision::new(revision),
    };
    execute_item_v1(
        &resolver,
        item(
            "checked",
            DirectItemCommandV1::Check(ItemCheckV1 {
                item_id: ItemId::new("checked").unwrap(),
                preconditions: item_fences(0),
            }),
        ),
        &MutationClock,
    )
    .unwrap();
    let read = |command| {
        execute_read_v1(
            &resolver,
            DirectReadRequestV1 {
                selector: selector.clone(),
                expected_workspace_id: None,
                expected_session_id: None,
                request_id: RequestIdV1::new(LOOKUP_RESPONSE).unwrap(),
                command,
                deadline: deadline(),
            },
            &Clock,
        )
        .unwrap()
    };
    let package = read(DirectReadCommandV1::DelegationPackage);
    let input = json!({"schema":"podway.delegation-accept-input/v1", "workspace_uuid":WORKSPACE, "session_id":LOOKUP_RESPONSE,
        "session_revision":2, "attempt_id":LOOKUP_RESPONSE, "idempotency_key":"accept", "package_digest":package.as_value()["result"]["package_digest"],
        "execution":{"binding_ref":"local-specialist", "adapter_id":"fixture", "adapter_version":"1", "primary_host":"external",
            "scope_kind":"test", "scope_ref":"scope-1", "supervisor_ref":"supervisor-1", "worker_ref":"worker-1", "task_ref":"task-1",
            "role_ref":"reviewer", "role_snapshot_digest":digest(), "agent_configuration_digest":digest(), "accepted_task_digest":digest()},
        "rounds_used":1, "result_digest":digest(), "summary":"Verified delegated work.", "worker_disposition":"idle", "accepted":true});
    let acceptance = |key: &str, input| DirectDelegationRequestV1 {
        selector: selector.clone(),
        expected_workspace_id: None,
        request_id: RequestIdV1::new(ORIGINAL_RESPONSE).unwrap(),
        key: IdempotencyKeyV1::new(key).unwrap(),
        input: DelegationAcceptV1 {
            input,
            preconditions: fences(),
        },
        deadline: deadline(),
    };
    let mut bad = input.clone();
    bad["idempotency_key"] = json!("bad-package");
    bad["package_digest"] = json!(format!("sha256:{}", "b".repeat(64)));
    assert!(matches!(
        accept_delegation_v1(
            &resolver,
            acceptance("bad-package", bad),
            &NoClock,
            &NativeDirectIdSourceV1
        ),
        Err(DirectExecutionErrorV1::Delegation(
            DirectDelegationFailureV1::BindingMismatch(_)
        ))
    ));
    let original_request = acceptance("accept", input);
    let accepted = accept_delegation_v1(
        &resolver,
        original_request.clone(),
        &MutationClock,
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    assert_eq!(
        accept_delegation_v1(
            &resolver,
            original_request,
            &NoClock,
            &NativeDirectIdSourceV1
        )
        .unwrap(),
        accepted
    );
    let receipt = read(DirectReadCommandV1::DelegationReceipt {
        session_id: SessionId::new(LOOKUP_RESPONSE).unwrap(),
        attempt_id: AttemptId::new(LOOKUP_RESPONSE).unwrap(),
    });
    assert_eq!(
        receipt.as_value()["result"]["receipt"]["receipt_id"],
        accepted.as_value()["result"]["receipt_id"]
    );
    execute_item_v1(
        &resolver,
        item(
            "changed-after-accept",
            DirectItemCommandV1::Set(ItemSetV1 {
                item_id: ItemId::new("note").unwrap(),
                value: "Changed after acceptance.".into(),
                preconditions: item_fences(0),
            }),
        ),
        &MutationClock,
    )
    .unwrap();
    assert!(matches!(
        execute_cursor_v1(
            &resolver,
            complete("changed-items"),
            &NoClock,
            &NativeDirectIdSourceV1
        ),
        Err(DirectExecutionErrorV1::Delegation(
            DirectDelegationFailureV1::ItemsChanged
        ))
    ));
    store
        .read(deadline(), |snapshot| {
            assert!(
                snapshot
                    .outcome(&IdempotencyKeyV1::new("missing-receipt").unwrap())?
                    .is_none()
            );
            assert!(
                snapshot
                    .outcome(&IdempotencyKeyV1::new("bad-package").unwrap())?
                    .is_none()
            );
            assert!(
                snapshot
                    .outcome(&IdempotencyKeyV1::new("changed-items").unwrap())?
                    .is_none()
            );
            assert_eq!(snapshot.committed_sequence()?, 5);
            Ok(())
        })
        .unwrap();
}

/// STATE-001 read-path pin: after an acceptance and a later change of one
/// accepted item, the same-snapshot view still reports `ItemsChanged` with
/// the receipt present and identity-valid inside the bounded window. The
/// accepted-items digest comparison is decided at the readiness stage, so
/// any window-derived readiness must keep evaluating it there.
#[test]
fn state001_changed_items_report_items_changed_through_view_with_in_window_receipt() {
    use podway_core::{AttemptId, ItemId, ProcedureSnapshotId, Revision, SessionId};
    use podway_engine::{
        direct_delegation::{DirectDelegationRequestV1, accept_delegation_v1},
        direct_execution::{
            DirectReadCommandV1, DirectReadRequestV1, NativeDirectIdSourceV1, execute_read_v1,
        },
        direct_items::{DirectItemCommandV1, DirectItemRequestV1, execute_item_v1},
        execution::workspace_procedure_snapshot_from_bytes_v2,
    };
    use podway_protocol::{
        DelegationAcceptV1, ItemCheckV1, ItemMutationPreconditionsWireV1, ItemSetV1,
        SessionMutationPreconditionsWireV1,
    };
    struct MutationClock;
    impl ExecutionClockV1 for MutationClock {
        fn now(&self) -> UnixMillis {
            UnixMillis::new(NOW + 20)
        }
    }
    let (_fixture, selector, store, _) = fixture();
    let snapshot = workspace_procedure_snapshot_from_bytes_v2(
        "delegated.json",
        &delegated_procedure(),
        ProcedureSnapshotId::new(LOOKUP_RESPONSE).unwrap(),
        UnixMillis::new(NOW + 2),
    )
    .unwrap();
    let graph = podway_store::GraphSessionStateV2::prepared(
        Revision::new(1),
        "Delegation fixture",
        snapshot,
        SessionId::new(LOOKUP_RESPONSE).unwrap(),
        UnixMillis::new(NOW + 2),
    )
    .unwrap()
    .begin_v2(
        Revision::ZERO,
        AttemptId::new(LOOKUP_RESPONSE).unwrap(),
        None,
        UnixMillis::new(NOW + 3),
    )
    .unwrap()
    .into_state();
    seed_graph(&store, &graph);
    let resolver = DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest());
    let fences = || {
        let graph = store
            .read(deadline(), |snapshot| snapshot.current_graph())
            .unwrap()
            .unwrap();
        SessionMutationPreconditionsWireV1 {
            expected_session_id: graph.trace().session_id().clone(),
            expected_session_revision: graph.trace().revision(),
            expected_attempt_id: graph.trace().active_attempt().unwrap().attempt_id().clone(),
        }
    };
    let read = |command| {
        execute_read_v1(
            &resolver,
            DirectReadRequestV1 {
                selector: selector.clone(),
                expected_workspace_id: None,
                expected_session_id: None,
                request_id: RequestIdV1::new(ORIGINAL_RESPONSE).unwrap(),
                command,
                deadline: deadline(),
            },
            &Clock,
        )
        .unwrap()
    };
    // Satisfy the delegated procedure's required confirm item first, as the
    // write-path precedent does, so acceptance is admissible; the package is
    // read after the check, matching that precedent's order.
    execute_item_v1(
        &resolver,
        DirectItemRequestV1 {
            selector: selector.clone(),
            expected_workspace_id: None,
            request_id: RequestIdV1::new(ORIGINAL_RESPONSE).unwrap(),
            key: IdempotencyKeyV1::new("view-checked").unwrap(),
            command: DirectItemCommandV1::Check(ItemCheckV1 {
                item_id: ItemId::new("checked").unwrap(),
                preconditions: ItemMutationPreconditionsWireV1 {
                    expected_session_id: SessionId::new(LOOKUP_RESPONSE).unwrap(),
                    expected_attempt_id: AttemptId::new(LOOKUP_RESPONSE).unwrap(),
                    expected_item_revision: Revision::new(0),
                },
            }),
            deadline: deadline(),
        },
        &MutationClock,
    )
    .unwrap();
    let package = read(DirectReadCommandV1::DelegationPackage);
    let current = store
        .read(deadline(), |snapshot| snapshot.current_graph())
        .unwrap()
        .unwrap();
    let input = json!({"schema":"podway.delegation-accept-input/v1", "workspace_uuid":WORKSPACE, "session_id":current.trace().session_id(),
        "session_revision":current.trace().revision(), "attempt_id":current.trace().active_attempt().unwrap().attempt_id(), "idempotency_key":"view-accept", "package_digest":package.as_value()["result"]["package_digest"],
        "execution":{"binding_ref":"local-specialist", "adapter_id":"fixture", "adapter_version":"1", "primary_host":"external",
            "scope_kind":"test", "scope_ref":"scope-1", "supervisor_ref":"supervisor-1", "worker_ref":"worker-1", "task_ref":"task-1",
            "role_ref":"reviewer", "role_snapshot_digest":digest(), "agent_configuration_digest":digest(), "accepted_task_digest":digest()},
        "rounds_used":1, "result_digest":digest(), "summary":"Verified delegated work.", "worker_disposition":"idle", "accepted":true});
    let acceptance = DirectDelegationRequestV1 {
        selector: selector.clone(),
        expected_workspace_id: None,
        request_id: RequestIdV1::new(ORIGINAL_RESPONSE).unwrap(),
        key: IdempotencyKeyV1::new("view-accept").unwrap(),
        input: DelegationAcceptV1 {
            input,
            preconditions: fences(),
        },
        deadline: deadline(),
    };
    accept_delegation_v1(
        &resolver,
        acceptance,
        &MutationClock,
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    // Change one accepted item on the same active attempt.
    execute_item_v1(
        &resolver,
        DirectItemRequestV1 {
            selector: selector.clone(),
            expected_workspace_id: None,
            request_id: RequestIdV1::new(ORIGINAL_RESPONSE).unwrap(),
            key: IdempotencyKeyV1::new("view-changed").unwrap(),
            command: DirectItemCommandV1::Set(ItemSetV1 {
                item_id: ItemId::new("note").unwrap(),
                value: "Changed after acceptance.".into(),
                preconditions: ItemMutationPreconditionsWireV1 {
                    expected_session_id: SessionId::new(LOOKUP_RESPONSE).unwrap(),
                    expected_attempt_id: AttemptId::new(LOOKUP_RESPONSE).unwrap(),
                    expected_item_revision: Revision::new(0),
                },
            }),
            deadline: deadline(),
        },
        &MutationClock,
    )
    .unwrap();
    let observed = store
        .read(deadline(), |snapshot| {
            let view = snapshot.view(UnixMillis::new(NOW + 30))?;
            Ok((
                view.delegation_receipts().readiness(),
                view.delegation_receipts().current_receipt().is_some(),
                view.delegation_receipts().truncated(),
                view.delegation_receipts().total(),
            ))
        })
        .unwrap();
    assert_eq!(
        observed,
        (
            podway_store::v2_delegation::DelegationReadinessV1::ItemsChanged,
            true,
            false,
            1
        )
    );
}

fn seed_running_items(store: &DirectStoreV1) {
    use podway_core::{AttemptId, ProcedureSnapshotId, Revision, SessionId};
    let snapshot = podway_engine::execution::workspace_procedure_snapshot_from_bytes_v2(
        "procedure.json",
        ITEMS,
        ProcedureSnapshotId::new(LOOKUP_RESPONSE).unwrap(),
        UnixMillis::new(NOW + 2),
    )
    .unwrap();
    let graph = podway_store::GraphSessionStateV2::prepared(
        Revision::new(1),
        "Item fixture",
        snapshot,
        SessionId::new(LOOKUP_RESPONSE).unwrap(),
        UnixMillis::new(NOW + 2),
    )
    .unwrap()
    .begin_v2(
        Revision::ZERO,
        AttemptId::new(LOOKUP_RESPONSE).unwrap(),
        None,
        UnixMillis::new(NOW + 3),
    )
    .unwrap()
    .into_state();
    seed_graph(store, &graph);
}

#[test]
fn item_commands_replay_success_after_attachment_changes_or_disappears() {
    use podway_core::{AttemptId, ItemId, Revision, SessionId};
    use podway_engine::direct_items::{
        DirectItemCommandV1 as C, DirectItemRequestV1, execute_item_v1,
    };
    use podway_protocol::{
        ItemAddV1, ItemAttachSourceV1, ItemAttachV1, ItemCheckV1, ItemClearV1,
        ItemMutationPreconditionsWireV1, ItemRemoveV1, ItemSetV1, ItemUncheckV1,
    };
    use podway_store::direct_store::DirectStoreErrorV1;
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct CountClock(AtomicUsize);
    impl ExecutionClockV1 for CountClock {
        fn now(&self) -> UnixMillis {
            UnixMillis::new(NOW + 20 + self.0.fetch_add(1, Ordering::SeqCst) as u64)
        }
    }
    let (fixture, selector, store, _) = fixture();
    seed_running_items(&store);
    let resolver = DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest());
    let clock = CountClock(AtomicUsize::new(0));
    let fences = |revision| ItemMutationPreconditionsWireV1 {
        expected_session_id: SessionId::new(LOOKUP_RESPONSE).unwrap(),
        expected_attempt_id: AttemptId::new(LOOKUP_RESPONSE).unwrap(),
        expected_item_revision: Revision::new(revision),
    };
    let input = |key: &str, command, response_id: &str| DirectItemRequestV1 {
        request_id: RequestIdV1::new(response_id).unwrap(),
        selector: selector.clone(),
        expected_workspace_id: Some(WorkspaceId::new(WORKSPACE).unwrap()),
        key: IdempotencyKeyV1::new(key).unwrap(),
        command,
        deadline: deadline(),
    };
    let commands = [
        C::Check(ItemCheckV1 {
            item_id: ItemId::new("checked").unwrap(),
            preconditions: fences(0),
        }),
        C::Uncheck(ItemUncheckV1 {
            item_id: ItemId::new("checked").unwrap(),
            preconditions: fences(1),
        }),
        C::Set(ItemSetV1 {
            item_id: ItemId::new("note").unwrap(),
            value: "Actual direct value".into(),
            preconditions: fences(0),
        }),
        C::Add(ItemAddV1 {
            item_id: ItemId::new("list").unwrap(),
            value: "one".into(),
            preconditions: fences(0),
        }),
        C::Remove(ItemRemoveV1 {
            item_id: ItemId::new("list").unwrap(),
            value: "one".into(),
            ignore_missing: false,
            preconditions: fences(1),
        }),
        C::Clear(ItemClearV1 {
            item_id: ItemId::new("note").unwrap(),
            preconditions: fences(1),
        }),
    ];
    for (index, command) in commands.into_iter().enumerate() {
        let key = format!("item-{index}");
        let output = execute_item_v1(
            &resolver,
            input(&key, command.clone(), ORIGINAL_RESPONSE),
            &clock,
        )
        .unwrap();
        assert!(validate_daemon_free_document_v1(
            "podway.output/v4",
            output.as_value()
        ));
        assert_eq!(output.as_value()["result"]["changed"], true);
        assert_eq!(
            execute_item_v1(&resolver, input(&key, command, LOOKUP_RESPONSE), &clock).unwrap(),
            output
        );
        assert_eq!(clock.0.load(Ordering::SeqCst), index + 1);
    }
    // A successful no-op has its own outcome and sequence but no graph revision.
    let no_op = C::Remove(ItemRemoveV1 {
        item_id: ItemId::new("list").unwrap(),
        value: "missing".into(),
        ignore_missing: true,
        preconditions: fences(2),
    });
    let output = execute_item_v1(
        &resolver,
        input("noop", no_op.clone(), ORIGINAL_RESPONSE),
        &clock,
    )
    .unwrap();
    assert_eq!(output.as_value()["result"]["changed"], false);
    assert_eq!(
        output.as_value()["session"]["revision_before"],
        output.as_value()["session"]["revision_after"]
    );
    assert_eq!(
        output.as_value()["workspace"]["latest_workspace_sequence"],
        9
    );
    assert_eq!(
        execute_item_v1(&resolver, input("noop", no_op, LOOKUP_RESPONSE), &clock).unwrap(),
        output
    );
    std::fs::write(fixture.path().join("report.txt"), "Original report").unwrap();
    let attach = C::Attach(ItemAttachV1 {
        item_id: ItemId::new("report").unwrap(),
        source: ItemAttachSourceV1::Path {
            path: "report.txt".into(),
            media_type: Some("text/plain".into()),
        },
        preconditions: fences(0),
    });
    let original = execute_item_v1(
        &resolver,
        input("attach", attach.clone(), ORIGINAL_RESPONSE),
        &clock,
    )
    .unwrap();
    for remove in [false, true] {
        if remove {
            std::fs::remove_file(fixture.path().join("report.txt")).unwrap();
        } else {
            std::fs::write(fixture.path().join("report.txt"), "Changed report").unwrap();
        }
        assert_eq!(
            execute_item_v1(
                &resolver,
                input("attach", attach.clone(), LOOKUP_RESPONSE),
                &clock
            )
            .unwrap(),
            original
        );
        assert_eq!(clock.0.load(Ordering::SeqCst), 8);
    }
    let changed = C::Attach(ItemAttachV1 {
        item_id: ItemId::new("report").unwrap(),
        source: ItemAttachSourceV1::Path {
            path: "missing-other.txt".into(),
            media_type: None,
        },
        preconditions: fences(0),
    });
    assert!(matches!(
        execute_item_v1(&resolver, input("attach", changed, LOOKUP_RESPONSE), &clock),
        Err(DirectExecutionErrorV1::Store(
            DirectStoreErrorV1::IdempotencyKeyReused
        ))
    ));
    let fresh = C::Attach(ItemAttachV1 {
        item_id: ItemId::new("report").unwrap(),
        source: ItemAttachSourceV1::Path {
            path: "report.txt".into(),
            media_type: None,
        },
        preconditions: fences(1),
    });
    assert!(matches!(
        execute_item_v1(
            &resolver,
            input("missing-new-key", fresh, LOOKUP_RESPONSE),
            &clock
        ),
        Err(DirectExecutionErrorV1::Boundary(_))
    ));
    assert_eq!(clock.0.load(Ordering::SeqCst), 8);
    store
        .read(deadline(), |snapshot| {
            assert_eq!(snapshot.committed_sequence()?, 10);
            assert!(
                snapshot
                    .outcome(&IdempotencyKeyV1::new("missing-new-key").unwrap())?
                    .is_none()
            );
            Ok(())
        })
        .unwrap();
}

#[test]
fn record_many_commits_ordered_items_atomically_and_replays_after_file_loss() {
    use podway_core::{AttemptId, ItemId, Revision, SessionId};
    use podway_engine::direct_items::{DirectRecordManyRequestV1, record_many_v1};
    use podway_protocol::{
        ItemRecordManyDispositionV1, ItemRecordManyOperationV1, ItemRecordManyV1,
        ItemRecordValueV1, SessionMutationPreconditionsWireV1,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct CountClock(AtomicUsize);
    impl ExecutionClockV1 for CountClock {
        fn now(&self) -> UnixMillis {
            UnixMillis::new(NOW + 20 + self.0.fetch_add(1, Ordering::SeqCst) as u64)
        }
    }
    let (fixture, selector, store, _) = fixture();
    seed_running_items(&store);
    std::fs::write(fixture.path().join("batch.txt"), "Batch evidence").unwrap();
    let operation = |id: &str, record| ItemRecordManyOperationV1 {
        item_id: ItemId::new(id).unwrap(),
        expected_item_revision: Revision::ZERO,
        disposition: ItemRecordManyDispositionV1::Record { record },
    };
    let input = ItemRecordManyV1 {
        operations: vec![
            operation(
                "note",
                ItemRecordValueV1::Text {
                    value: "batch note".into(),
                },
            ),
            operation("checked", ItemRecordValueV1::Confirm { value: true }),
            operation(
                "list",
                ItemRecordValueV1::List {
                    value: vec!["first".into(), "second".into()],
                },
            ),
            operation(
                "report",
                ItemRecordValueV1::Artifact {
                    path: Some("batch.txt".into()),
                    reference: None,
                    digest: None,
                    size_bytes: None,
                    media_type: Some("text/plain".into()),
                },
            ),
        ],
        preconditions: SessionMutationPreconditionsWireV1 {
            expected_session_id: SessionId::new(LOOKUP_RESPONSE).unwrap(),
            expected_session_revision: Revision::new(1),
            expected_attempt_id: AttemptId::new(LOOKUP_RESPONSE).unwrap(),
        },
    };
    let request = |key: &str, input, response: &str| DirectRecordManyRequestV1 {
        request_id: RequestIdV1::new(response).unwrap(),
        selector: selector.clone(),
        expected_workspace_id: Some(WorkspaceId::new(WORKSPACE).unwrap()),
        key: IdempotencyKeyV1::new(key).unwrap(),
        input,
        deadline: deadline(),
    };
    let resolver = DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest());
    let clock = CountClock(AtomicUsize::new(0));
    let output = record_many_v1(
        &resolver,
        request("batch", input.clone(), ORIGINAL_RESPONSE),
        &clock,
    )
    .unwrap();
    assert!(validate_daemon_free_document_v1(
        "podway.output/v4",
        output.as_value()
    ));
    assert_eq!(output.as_value()["result"]["revision"], 2);
    assert_eq!(
        output.as_value()["workspace"]["latest_workspace_sequence"],
        3
    );
    assert_eq!(
        output.as_value()["result"]["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["item_id"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["checked", "list", "note", "report"]
    );
    assert_eq!(clock.0.load(Ordering::SeqCst), 1);
    std::fs::remove_file(fixture.path().join("batch.txt")).unwrap();
    assert_eq!(
        record_many_v1(
            &resolver,
            request("batch", input.clone(), LOOKUP_RESPONSE),
            &clock
        )
        .unwrap(),
        output
    );
    assert_eq!(clock.0.load(Ordering::SeqCst), 1);
    let mut reversed = input.clone();
    reversed.operations.reverse();
    assert_eq!(
        record_many_v1(
            &resolver,
            request("batch", reversed, LOOKUP_RESPONSE),
            &clock
        )
        .unwrap(),
        output
    );
    let mut bad = input;
    bad.operations = vec![
        operation(
            "note",
            ItemRecordValueV1::Text {
                value: "must roll back".into(),
            },
        ),
        operation(
            "checked",
            ItemRecordValueV1::Text {
                value: "wrong type".into(),
            },
        ),
    ];
    for operation in &mut bad.operations {
        operation.expected_item_revision = Revision::new(1);
    }
    bad.preconditions.expected_session_revision = Revision::new(2);
    assert!(
        record_many_v1(
            &resolver,
            request("invalid-batch", bad.clone(), LOOKUP_RESPONSE),
            &clock
        )
        .is_err()
    );
    store
        .read(deadline(), |snapshot| {
            assert_eq!(snapshot.committed_sequence()?, 3);
            assert_eq!(
                snapshot.current_graph()?.unwrap().trace().revision(),
                Revision::new(2)
            );
            assert!(
                snapshot
                    .outcome(&IdempotencyKeyV1::new("invalid-batch").unwrap())?
                    .is_none()
            );
            Ok(())
        })
        .unwrap();
    bad.operations[1].expected_item_revision = Revision::new(9);
    let calls = clock.0.load(Ordering::SeqCst);
    assert!(
        record_many_v1(
            &resolver,
            request("stale-batch", bad, LOOKUP_RESPONSE),
            &clock
        )
        .is_err()
    );
    assert_eq!(clock.0.load(Ordering::SeqCst), calls);
}

#[test]
fn typed_record_many_rejects_count_and_duplicates_before_write_admission() {
    use podway_core::{AttemptId, ItemId, Revision, SessionId};
    use podway_engine::direct_items::{DirectRecordManyRequestV1, record_many_v1};
    use podway_protocol::{
        ItemRecordManyDispositionV1, ItemRecordManyOperationV1, ItemRecordManyV1,
        ItemRecordValueV1, SessionMutationPreconditionsWireV1,
    };
    use podway_store::direct_store::{DirectStoreErrorV1, DirectTransactionBoundaryV1};
    struct NoClock;
    impl ExecutionClockV1 for NoClock {
        fn now(&self) -> UnixMillis {
            panic!("invalid typed batch read the mutation clock")
        }
    }
    let (fixture, selector, store, _) = fixture();
    seed_running_items(&store);
    std::fs::write(fixture.path().join("report.txt"), "Batch evidence").unwrap();
    let operation = |id: &str| ItemRecordManyOperationV1 {
        item_id: ItemId::new(id).unwrap(),
        expected_item_revision: Revision::ZERO,
        disposition: ItemRecordManyDispositionV1::Record {
            record: ItemRecordValueV1::Artifact {
                path: Some("report.txt".into()),
                reference: None,
                digest: None,
                size_bytes: None,
                media_type: Some("text/plain".into()),
            },
        },
    };
    let resolver = DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest())
        .with_store_boundary_observer_v1(|boundary| {
            if boundary == DirectTransactionBoundaryV1::BeforeBegin {
                panic!("invalid typed batch entered write admission");
            }
        });
    let before = store
        .read(deadline(), |snapshot| snapshot.current_graph())
        .unwrap();
    for (key, operations, expected) in [
        ("empty", vec![], "batch requires 1 through 128 operations"),
        (
            "too-many",
            (0..129).map(|i| operation(&format!("item-{i}"))).collect(),
            "batch requires 1 through 128 operations",
        ),
        (
            "duplicates",
            vec![operation("report"), operation("report")],
            "duplicate batch item",
        ),
    ] {
        let key = IdempotencyKeyV1::new(key).unwrap();
        let error = record_many_v1(
            &resolver,
            DirectRecordManyRequestV1 {
                request_id: RequestIdV1::new(ORIGINAL_RESPONSE).unwrap(),
                selector: selector.clone(),
                expected_workspace_id: Some(WorkspaceId::new(WORKSPACE).unwrap()),
                key: key.clone(),
                deadline: deadline(),
                input: ItemRecordManyV1 {
                    operations,
                    preconditions: SessionMutationPreconditionsWireV1 {
                        expected_session_id: SessionId::new(LOOKUP_RESPONSE).unwrap(),
                        expected_session_revision: Revision::new(1),
                        expected_attempt_id: AttemptId::new(LOOKUP_RESPONSE).unwrap(),
                    },
                },
            },
            &NoClock,
        )
        .unwrap_err();
        assert!(
            matches!(error, DirectExecutionErrorV1::Store(DirectStoreErrorV1::InvalidInput { reason }) if reason == expected)
        );
        store
            .read(deadline(), |snapshot| {
                assert_eq!(snapshot.current_graph()?, before);
                assert_eq!(snapshot.committed_sequence()?, 2);
                assert!(snapshot.outcome(&key)?.is_none());
                Ok(())
            })
            .unwrap();
    }
}

#[test]
fn record_many_enforces_aggregate_encoded_input_bound_without_effects_and_preserves_replay() {
    use podway_core::{AttemptId, ItemId, ProcedureSnapshotId, Revision, SessionId};
    use podway_engine::{
        direct_api::{DirectCommandV1, DirectRequestV1, DirectResponseV1, execute_direct_v1},
        direct_error::{DirectErrorContextV1, render_direct_error_v1},
        direct_execution::NativeDirectIdSourceV1,
        direct_items::{DirectRecordManyRequestV1, record_many_v1},
        execution::workspace_procedure_snapshot_from_bytes_v2,
    };
    use podway_protocol::{
        CommandNameV1, ItemRecordManyDispositionV1, ItemRecordManyInputV1,
        ItemRecordManyOperationV1, ItemRecordManyV1, ItemRecordValueV1, MAX_DIRECT_INPUT_BYTES_V1,
        Rfc3339MillisV1, SessionMutationPreconditionsWireV1,
    };
    use podway_store::GraphSessionStateV2;
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct CountClock(AtomicUsize);
    impl ExecutionClockV1 for CountClock {
        fn now(&self) -> UnixMillis {
            UnixMillis::new(NOW + 20 + self.0.fetch_add(1, Ordering::SeqCst) as u64)
        }
    }
    for through_facade in [false, true] {
        let (_fixture, selector, store, _) = fixture();
        let mut procedure: serde_json::Value = serde_json::from_slice(SIMPLE).unwrap();
        procedure["node_definitions"]["work"]["items"] = json!(
            (0..16)
                .map(|n| {
                    json!({"id":format!("item-{n}"),"type":"text","prompt":"Record text.",
                "required":false,"max_length":65536})
                })
                .collect::<Vec<_>>()
        );
        let snapshot = workspace_procedure_snapshot_from_bytes_v2(
            "procedure.json",
            &serde_json::to_vec(&procedure).unwrap(),
            ProcedureSnapshotId::new(LOOKUP_RESPONSE).unwrap(),
            UnixMillis::new(NOW + 2),
        )
        .unwrap();
        let graph = GraphSessionStateV2::prepared(
            Revision::new(1),
            "Input budget",
            snapshot,
            SessionId::new(LOOKUP_RESPONSE).unwrap(),
            UnixMillis::new(NOW + 2),
        )
        .unwrap()
        .begin_v2(
            Revision::ZERO,
            AttemptId::new(LOOKUP_RESPONSE).unwrap(),
            None,
            UnixMillis::new(NOW + 3),
        )
        .unwrap()
        .into_state();
        seed_graph(&store, &graph);
        let resolver = DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest());
        let clock = CountClock(AtomicUsize::new(0));
        let input = ItemRecordManyV1 {
            operations: (0..16)
                .map(|n| ItemRecordManyOperationV1 {
                    item_id: ItemId::new(format!("item-{n}")).unwrap(),
                    expected_item_revision: Revision::ZERO,
                    disposition: ItemRecordManyDispositionV1::Record {
                        record: ItemRecordValueV1::Text {
                            value: "x".repeat(65536),
                        },
                    },
                })
                .collect(),
            preconditions: SessionMutationPreconditionsWireV1 {
                expected_session_id: graph.trace().session_id().clone(),
                expected_session_revision: graph.trace().revision(),
                expected_attempt_id: graph.trace().active_attempt().unwrap().attempt_id().clone(),
            },
        };
        let encoded_size = |input: &ItemRecordManyV1, key: &str| {
            serde_json::to_vec(&ItemRecordManyInputV1 {
                schema: "podway.item-record-many-input/v1".into(),
                workspace_uuid: WorkspaceId::new(WORKSPACE).unwrap(),
                session_id: input.preconditions.expected_session_id.clone(),
                session_revision: input.preconditions.expected_session_revision,
                attempt_id: input.preconditions.expected_attempt_id.clone(),
                idempotency_key: podway_protocol::IdempotencyKeyV1::new(key).unwrap(),
                operations: input.operations.clone(),
            })
            .unwrap()
            .len()
        };
        let run = |input: ItemRecordManyV1, key: &str, request_id: &str| {
            let request_id = RequestIdV1::new(request_id).unwrap();
            let key = IdempotencyKeyV1::new(key).unwrap();
            let observed_at = Rfc3339MillisV1::from_unix_millis(NOW).unwrap();
            let response = if through_facade {
                execute_direct_v1(
                    &resolver,
                    DirectRequestV1 {
                        request_id,
                        selector: selector.clone(),
                        expected_workspace_id: Some(WorkspaceId::new(WORKSPACE).unwrap()),
                        command: DirectCommandV1::RecordMany(input),
                        idempotency_key: Some(key),
                        deadline: deadline(),
                        observed_at,
                        timeout_ms: 5000,
                    },
                    &clock,
                    &NativeDirectIdSourceV1,
                )
                .unwrap()
            } else {
                let context = DirectErrorContextV1 {
                    request_id: request_id.clone(),
                    command: CommandNameV1::new("item.record_many").unwrap(),
                    observed_at,
                    idempotency_key: Some(key.clone()),
                    timeout_ms: 5000,
                    maintenance: None,
                };
                match record_many_v1(
                    &resolver,
                    DirectRecordManyRequestV1 {
                        request_id,
                        selector: selector.clone(),
                        expected_workspace_id: Some(WorkspaceId::new(WORKSPACE).unwrap()),
                        key,
                        input,
                        deadline: deadline(),
                    },
                    &clock,
                ) {
                    Ok(output) => DirectResponseV1::Success(output),
                    Err(error) => {
                        DirectResponseV1::Failure(render_direct_error_v1(&error, &context).unwrap())
                    }
                }
            };
            match response {
                DirectResponseV1::Success(output) => Ok(output),
                DirectResponseV1::Failure(error) => {
                    let value = error.into_value();
                    assert!(validate_daemon_free_document_v1("podway.error/v2", &value));
                    Err(value["code"].as_str().unwrap().to_owned())
                }
            }
        };
        let key = "input-bound";
        let extra = encoded_size(&input, key) - MAX_DIRECT_INPUT_BYTES_V1;
        assert!(extra > 0 && extra < 65536);
        let mut boundary = input.clone();
        let ItemRecordManyDispositionV1::Record {
            record: ItemRecordValueV1::Text { value },
        } = &mut boundary.operations[15].disposition
        else {
            unreachable!()
        };
        value.truncate(65536 - extra);
        assert_eq!(encoded_size(&boundary, key), MAX_DIRECT_INPUT_BYTES_V1);
        let mut one_over = boundary.clone();
        let ItemRecordManyDispositionV1::Record {
            record: ItemRecordValueV1::Text { value },
        } = &mut one_over.operations[15].disposition
        else {
            unreachable!()
        };
        value.push('x');
        assert_eq!(encoded_size(&one_over, key), MAX_DIRECT_INPUT_BYTES_V1 + 1);
        let mut escaped = input.clone();
        for operation in &mut escaped.operations {
            operation.disposition = ItemRecordManyDispositionV1::Record {
                record: ItemRecordValueV1::Text {
                    value: "\0".repeat(16000),
                },
            };
        }
        assert!(encoded_size(&escaped, key) > MAX_DIRECT_INPUT_BYTES_V1);
        for oversized in [input, one_over, escaped] {
            assert_eq!(
                run(oversized, key, ORIGINAL_RESPONSE).unwrap_err(),
                "REQUEST_TOO_LARGE"
            );
            assert_eq!(clock.0.load(Ordering::SeqCst), 0);
            store
                .read(deadline(), |snapshot| {
                    assert_eq!(snapshot.committed_sequence()?, 2);
                    assert_eq!(snapshot.current_graph()?.unwrap(), graph);
                    assert!(
                        snapshot
                            .outcome(&IdempotencyKeyV1::new(key).unwrap())?
                            .is_none()
                    );
                    Ok(())
                })
                .unwrap();
        }
        let output = run(boundary.clone(), key, ORIGINAL_RESPONSE).unwrap();
        assert!(validate_daemon_free_document_v1(
            "podway.output/v4",
            output.as_value()
        ));
        assert_eq!(
            output.as_value()["workspace"]["latest_workspace_sequence"],
            3
        );
        assert_eq!(clock.0.load(Ordering::SeqCst), 1);
        boundary.operations.reverse();
        assert_eq!(run(boundary, key, LOOKUP_RESPONSE).unwrap(), output);
        assert_eq!(clock.0.load(Ordering::SeqCst), 1);
    }
}

#[test]
fn direct_goal_decision_rework_completion_and_disposition_preserve_records_and_replay() {
    use podway_core::{
        AttemptId, CriterionId, GraphNodeId, ItemId, OptionId, ProcedureSnapshotId, Revision,
        SessionId,
    };
    use podway_engine::{
        direct_execution::{
            DirectCursorCommandV1, DirectCursorRequestV1, NativeDirectIdSourceV1, execute_cursor_v1,
        },
        direct_items::{DirectItemCommandV1, DirectItemRequestV1, execute_item_v1},
        direct_procedure::{DirectProcedureRequestV1, execute_procedure_v1},
        execution::workspace_procedure_snapshot_from_bytes_v2,
    };
    use podway_protocol::{
        GoalAssessCriterionV2, GoalCriterionWireV2, GoalDefineV2, GoalReviseV2,
        GoalRevisionPreconditionsWireV2, ItemMutationPreconditionsWireV1, ItemSetV1,
        ProcedureV2MutationCommandV1 as C, ReworkPreconditionsWireV2, SessionCompleteV1,
        SessionDecideV2, SessionIdentityPreconditionsWireV1, SessionMutationPreconditionsWireV1,
        SessionReworkV2, SessionTerminalDispositionV1, TerminalDispositionInputV1,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct CountClock(AtomicUsize);
    impl ExecutionClockV1 for CountClock {
        fn now(&self) -> UnixMillis {
            UnixMillis::new(NOW + 20 + self.0.fetch_add(1, Ordering::SeqCst) as u64)
        }
    }
    let (_fixture, selector, store, _) = fixture();
    let snapshot = workspace_procedure_snapshot_from_bytes_v2(
        "procedure.json",
        include_bytes!("../../../tests/fixtures/v2/procedures/equivalent-procedure.json"),
        ProcedureSnapshotId::new(LOOKUP_RESPONSE).unwrap(),
        UnixMillis::new(NOW + 2),
    )
    .unwrap();
    let graph = podway_store::GraphSessionStateV2::prepared(
        Revision::new(1),
        "Goal fixture",
        snapshot,
        SessionId::new(LOOKUP_RESPONSE).unwrap(),
        UnixMillis::new(NOW + 2),
    )
    .unwrap()
    .begin_v2(
        Revision::ZERO,
        AttemptId::new(LOOKUP_RESPONSE).unwrap(),
        None,
        UnixMillis::new(NOW + 3),
    )
    .unwrap()
    .into_state();
    seed_graph(&store, &graph);
    let resolver = DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest());
    let clock = CountClock(AtomicUsize::new(0));
    let ids = NativeDirectIdSourceV1;
    let current = || {
        store
            .read(deadline(), |snapshot| snapshot.current_graph())
            .unwrap()
            .unwrap()
    };
    let criteria = || {
        vec![GoalCriterionWireV2 {
            criterion_id: CriterionId::new("done").unwrap(),
            statement: "The result is verified.".into(),
        }]
    };
    let session_fences = || SessionIdentityPreconditionsWireV1 {
        expected_session_id: current().trace().session_id().clone(),
        expected_session_revision: current().trace().revision(),
    };
    let cursor_fences = || {
        let graph = current();
        SessionMutationPreconditionsWireV1 {
            expected_session_id: graph.trace().session_id().clone(),
            expected_session_revision: graph.trace().revision(),
            expected_attempt_id: graph.trace().active_attempt().unwrap().attempt_id().clone(),
        }
    };
    let procedure = |key: &str, command: C| {
        let request = |response| DirectProcedureRequestV1 {
            selector: selector.clone(),
            expected_workspace_id: Some(WorkspaceId::new(WORKSPACE).unwrap()),
            request_id: RequestIdV1::new(response).unwrap(),
            key: IdempotencyKeyV1::new(key).unwrap(),
            command: command.clone(),
            deadline: deadline(),
        };
        let output =
            execute_procedure_v1(&resolver, request(ORIGINAL_RESPONSE), &clock, &ids).unwrap();
        let calls = clock.0.load(Ordering::SeqCst);
        assert_eq!(
            execute_procedure_v1(&resolver, request(LOOKUP_RESPONSE), &clock, &ids).unwrap(),
            output
        );
        assert_eq!(clock.0.load(Ordering::SeqCst), calls);
        assert!(validate_daemon_free_document_v1(
            "podway.output/v4",
            output.as_value()
        ));
        output
    };
    procedure(
        "define",
        C::GoalDefine(GoalDefineV2 {
            goal: "Verify the direct flow.".into(),
            criteria: criteria(),
            actor: Some("coordinator".into()),
            preconditions: session_fences(),
        }),
    );
    let fences = cursor_fences();
    procedure(
        "revise",
        C::GoalRevise(GoalReviseV2 {
            goal: "Verify the revised direct flow.".into(),
            criteria: criteria(),
            target_graph_node_id: GraphNodeId::new("perform").unwrap(),
            reason: "Adjust the objective.".into(),
            actor: Some("coordinator".into()),
            reactivate: false,
            preconditions: GoalRevisionPreconditionsWireV2 {
                expected_session_id: fences.expected_session_id,
                expected_session_revision: fences.expected_session_revision,
                expected_attempt_id: Some(fences.expected_attempt_id),
                expected_goal_revision: 1,
            },
        }),
    );
    let fences = cursor_fences();
    procedure(
        "rework",
        C::SessionRework(SessionReworkV2 {
            target_graph_node_id: GraphNodeId::new("perform").unwrap(),
            reason: "Repeat the work.".into(),
            actor: Some("coordinator".into()),
            preconditions: ReworkPreconditionsWireV2 {
                expected_session_id: fences.expected_session_id,
                expected_session_revision: fences.expected_session_revision,
                expected_attempt_id: Some(fences.expected_attempt_id),
            },
        }),
    );
    let set_result = |key: &str| {
        let graph = current();
        execute_item_v1(
            &resolver,
            DirectItemRequestV1 {
                selector: selector.clone(),
                expected_workspace_id: None,
                request_id: RequestIdV1::new(ORIGINAL_RESPONSE).unwrap(),
                key: IdempotencyKeyV1::new(key).unwrap(),
                deadline: deadline(),
                command: DirectItemCommandV1::Set(ItemSetV1 {
                    item_id: ItemId::new("result").unwrap(),
                    value: "Verified result".into(),
                    preconditions: ItemMutationPreconditionsWireV1 {
                        expected_session_id: graph.trace().session_id().clone(),
                        expected_attempt_id: graph
                            .trace()
                            .active_attempt()
                            .unwrap()
                            .attempt_id()
                            .clone(),
                        expected_item_revision: Revision::ZERO,
                    },
                }),
            },
            &clock,
        )
        .unwrap()
    };
    let complete = |key: &str| {
        let command = DirectCursorCommandV1::Complete(SessionCompleteV1 {
            preconditions: cursor_fences(),
        });
        let request = |response| DirectCursorRequestV1 {
            selector: selector.clone(),
            expected_workspace_id: None,
            request_id: RequestIdV1::new(response).unwrap(),
            key: IdempotencyKeyV1::new(key).unwrap(),
            command: command.clone(),
            deadline: deadline(),
        };
        let output =
            execute_cursor_v1(&resolver, request(ORIGINAL_RESPONSE), &clock, &ids).unwrap();
        let calls = clock.0.load(Ordering::SeqCst);
        assert_eq!(
            execute_cursor_v1(&resolver, request(LOOKUP_RESPONSE), &clock, &ids).unwrap(),
            output
        );
        assert_eq!(clock.0.load(Ordering::SeqCst), calls);
        output
    };
    set_result("perform-result");
    complete("perform-complete");
    procedure(
        "assess",
        C::GoalAssessCriterion(GoalAssessCriterionV2 {
            criterion_id: CriterionId::new("done").unwrap(),
            status: "satisfied".into(),
            reason: "The evidence verifies the result.".into(),
            evidence: vec![GraphNodeId::new("perform").unwrap()],
            items: Vec::new(),
            actor: Some("coordinator".into()),
            preconditions: cursor_fences(),
            expected_goal_revision: 2,
        }),
    );
    let decision = procedure(
        "decide",
        C::SessionDecide(SessionDecideV2 {
            option_id: OptionId::new("achieved").unwrap(),
            reason: "The goal is achieved.".into(),
            actor: Some("coordinator".into()),
            preconditions: cursor_fences(),
            expected_goal_revision: Some(2),
        }),
    );
    assert_eq!(
        decision.as_value()["result"]["record"]["goal_outcome"],
        "achieved"
    );
    set_result("finish-result");
    complete("finish-complete");
    let disposition = procedure(
        "dispose",
        C::SessionTerminalDisposition(SessionTerminalDispositionV1 {
            disposition: TerminalDispositionInputV1::HandedOff {
                summary: "Verified all direct routes.".into(),
                reference: "commit:fixture".into(),
            },
            actor: Some("coordinator".into()),
            preconditions: session_fences(),
        }),
    );
    assert_eq!(
        disposition.as_value()["session"]["revision_before"],
        disposition.as_value()["session"]["revision_after"]
    );
    assert_eq!(clock.0.load(Ordering::SeqCst), 10);
    store
        .read(deadline(), |snapshot| {
            let view = snapshot.view(UnixMillis::new(NOW + 40))?;
            assert_eq!(view.sequence(), 12);
            assert_eq!(view.terminal_dispositions().len(), 1);
            let graph = view.graph().unwrap();
            assert_eq!(graph.goal_state().revisions().len(), 2);
            assert_eq!(
                graph.trace().lifecycle(),
                podway_core::SessionLifecycle::Completed
            );
            Ok(())
        })
        .unwrap();
    use podway_engine::{
        direct_execution::{DirectReadCommandV1, DirectReadRequestV1, execute_read_v1},
        direct_sessions::{DirectSessionCommandV1, DirectSessionRequestV1, execute_session_v1},
        v2_read_service::GraphStatusTierV2,
    };
    use podway_protocol::{SessionArchivePurgeV1, SessionArchiveV1};
    let command = DirectSessionCommandV1::Archive(SessionArchiveV1 {
        preconditions: session_fences(),
    });
    let storage_request = |key: &str, command, response: &str| DirectSessionRequestV1 {
        selector: selector.clone(),
        expected_workspace_id: None,
        request_id: RequestIdV1::new(response).unwrap(),
        key: Some(IdempotencyKeyV1::new(key).unwrap()),
        command,
        deadline: deadline(),
    };
    let archived = execute_session_v1(
        &resolver,
        storage_request("archive", command.clone(), ORIGINAL_RESPONSE),
        &clock,
    )
    .unwrap();
    assert_eq!(
        execute_session_v1(
            &resolver,
            storage_request("archive", command.clone(), LOOKUP_RESPONSE),
            &clock
        )
        .unwrap(),
        archived
    );
    let read = |command| {
        execute_read_v1(
            &resolver,
            DirectReadRequestV1 {
                selector: selector.clone(),
                expected_workspace_id: None,
                expected_session_id: None,
                request_id: RequestIdV1::new(LOOKUP_RESPONSE).unwrap(),
                command,
                deadline: deadline(),
            },
            &Clock,
        )
        .unwrap()
    };
    assert_eq!(
        read(DirectReadCommandV1::ArchiveList).as_value()["result"]["count"],
        1
    );
    let shown = read(DirectReadCommandV1::ArchiveShow {
        session_id: SessionId::new(LOOKUP_RESPONSE).unwrap(),
        tier: GraphStatusTierV2::Verbose,
        history_before: None,
    });
    assert_eq!(
        shown.as_value()["result"]["state"]["session"]["lifecycle"],
        "completed"
    );
    assert!(shown.as_value()["result"]["state"].get("queue").is_none());
    let purge = DirectSessionCommandV1::Purge(SessionArchivePurgeV1 {
        session_id: SessionId::new(LOOKUP_RESPONSE).unwrap(),
        expected_session_revision: Revision::new(10),
        confirmed: true,
    });
    let purged = execute_session_v1(
        &resolver,
        storage_request("purge", purge.clone(), ORIGINAL_RESPONSE),
        &clock,
    )
    .unwrap();
    assert_eq!(
        execute_session_v1(
            &resolver,
            storage_request("purge", purge, LOOKUP_RESPONSE),
            &clock
        )
        .unwrap(),
        purged
    );
    assert_eq!(
        execute_session_v1(
            &resolver,
            storage_request("archive", command, LOOKUP_RESPONSE),
            &clock
        )
        .unwrap(),
        archived
    );
    assert_eq!(
        read(DirectReadCommandV1::ArchiveList).as_value()["result"]["count"],
        0
    );
    let purge_lookup =
        lookup_outcome_v1(&resolver, request(selector.clone(), "purge"), &Clock).unwrap();
    assert_eq!(
        purge_lookup.as_value()["result"]["outcome"]["scope"],
        "workspace"
    );
    let catalog: serde_norway::Value = serde_norway::from_str(include_str!(
        "../../../assets/specifications/command-catalog.yaml"
    ))
    .unwrap();
    for (name, key) in [
        ("session.archive", "archive"),
        ("session.archive_purge", "purge"),
    ] {
        let declared = catalog["commands"]
            .as_sequence()
            .unwrap()
            .iter()
            .find(|entry| entry["name"].as_str() == Some(name))
            .unwrap();
        let actual = lookup_outcome_v1(&resolver, request(selector.clone(), key), &Clock).unwrap();
        assert_eq!(
            declared["idempotency_scope"].as_str(),
            actual.as_value()["result"]["outcome"]["scope"].as_str(),
            "canonical catalog scope differs from the retained Engine outcome for {name}"
        );
    }

    assert_eq!(
        purge_lookup.as_value()["result"]["outcome"]["response"],
        *purged.as_value()
    );
    let looked_up =
        lookup_outcome_v1(&resolver, request(selector.clone(), "decide"), &Clock).unwrap();
    assert_eq!(
        looked_up.as_value()["result"]["outcome"]["response"],
        *decision.as_value()
    );
    assert_eq!(clock.0.load(Ordering::SeqCst), 12);

    struct AfterRetention;
    impl ExecutionClockV1 for AfterRetention {
        fn now(&self) -> UnixMillis {
            UnixMillis::new(NOW + 31 * 24 * 60 * 60 * 1000)
        }
    }
    use podway_engine::direct_start::{
        DirectStartCommandV1, DirectStartRequestV1, start_session_v1,
    };
    use podway_protocol::{SessionStartSourceV1, SessionStartV1};
    start_session_v1(
        &resolver,
        DirectStartRequestV1 {
            selector: selector.clone(),
            expected_workspace_id: Some(WorkspaceId::new(WORKSPACE).unwrap()),
            request_id: RequestIdV1::new(ORIGINAL_RESPONSE).unwrap(),
            key: Some(IdempotencyKeyV1::new("after-retention").unwrap()),
            command: DirectStartCommandV1::Start(SessionStartV1 {
                source: SessionStartSourceV1::Preset {
                    preset: "small-change-v2".into(),
                },
                expected_procedure_digest: None,
                task_title: "After workspace retention".into(),
                dry_run: false,
            }),
            deadline: deadline(),
        },
        &AfterRetention,
        &ids,
    )
    .unwrap();
    let expired = lookup_outcome_v1(
        &resolver,
        request(selector.clone(), "purge"),
        &AfterRetention,
    )
    .unwrap();
    assert_eq!(expired.as_value()["result"]["found"], false);
    assert!(expired.as_value()["result"]["outcome"].is_null());
    for (key, original) in [("decide", decision), ("archive", archived)] {
        let retained =
            lookup_outcome_v1(&resolver, request(selector.clone(), key), &AfterRetention).unwrap();
        assert_eq!(retained.as_value()["result"]["outcome"]["scope"], "session");
        assert_eq!(
            retained.as_value()["result"]["outcome"]["response"],
            *original.as_value()
        );
    }
}

#[test]
fn direct_reset_preview_and_force_remove_only_session_scoped_outcomes() {
    struct MutationClock;
    impl ExecutionClockV1 for MutationClock {
        fn now(&self) -> UnixMillis {
            UnixMillis::new(NOW + 20)
        }
    }
    use podway_core::{AttemptId, ItemId, Revision, SessionId};
    use podway_engine::{
        direct_items::{DirectItemCommandV1, DirectItemRequestV1, execute_item_v1},
        direct_sessions::{DirectSessionCommandV1, DirectSessionRequestV1, execute_session_v1},
    };
    use podway_protocol::{
        ItemMutationPreconditionsWireV1, ItemSetV1, SessionDeletionModeV1,
        SessionIdentityPreconditionsWireV1, SessionResetV1,
    };
    let (_fixture, selector, store, _) = fixture();
    seed_running_items(&store);
    let resolver = DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest());
    execute_item_v1(
        &resolver,
        DirectItemRequestV1 {
            selector: selector.clone(),
            expected_workspace_id: None,
            request_id: RequestIdV1::new(ORIGINAL_RESPONSE).unwrap(),
            key: IdempotencyKeyV1::new("before-reset").unwrap(),
            deadline: deadline(),
            command: DirectItemCommandV1::Set(ItemSetV1 {
                item_id: ItemId::new("note").unwrap(),
                value: "Session scoped value".into(),
                preconditions: ItemMutationPreconditionsWireV1 {
                    expected_session_id: SessionId::new(LOOKUP_RESPONSE).unwrap(),
                    expected_attempt_id: AttemptId::new(LOOKUP_RESPONSE).unwrap(),
                    expected_item_revision: Revision::ZERO,
                },
            }),
        },
        &MutationClock,
    )
    .unwrap();
    let fences = SessionIdentityPreconditionsWireV1 {
        expected_session_id: SessionId::new(LOOKUP_RESPONSE).unwrap(),
        expected_session_revision: Revision::new(2),
    };
    let input = |key, mode, dry_run| DirectSessionRequestV1 {
        selector: selector.clone(),
        expected_workspace_id: None,
        request_id: RequestIdV1::new(ORIGINAL_RESPONSE).unwrap(),
        key,
        deadline: deadline(),
        command: DirectSessionCommandV1::Reset(SessionResetV1 {
            mode,
            dry_run,
            preconditions: fences.clone(),
        }),
    };
    let preview = execute_session_v1(
        &resolver,
        input(None, SessionDeletionModeV1::Eligible, true),
        &MutationClock,
    )
    .unwrap();
    assert_eq!(preview.as_value()["result"]["required_action"], "force");
    assert_eq!(preview.as_value()["result"]["reset"], false);
    struct NoClock;
    impl ExecutionClockV1 for NoClock {
        fn now(&self) -> UnixMillis {
            panic!("ineligible reset read the mutation clock")
        }
    }
    assert!(
        execute_session_v1(
            &resolver,
            input(
                Some(IdempotencyKeyV1::new("ineligible").unwrap()),
                SessionDeletionModeV1::Eligible,
                false
            ),
            &NoClock
        )
        .is_err()
    );
    let force = input(
        Some(IdempotencyKeyV1::new("reset").unwrap()),
        SessionDeletionModeV1::Force {
            progress_summary: "x".repeat(3000),
        },
        false,
    );
    let output = execute_session_v1(&resolver, force.clone(), &MutationClock).unwrap();
    assert_eq!(
        execute_session_v1(&resolver, force, &NoClock).unwrap(),
        output
    );
    store
        .read(deadline(), |snapshot| {
            assert!(snapshot.current_graph()?.is_none());
            assert!(
                snapshot
                    .outcome(&IdempotencyKeyV1::new("before-reset").unwrap())?
                    .is_none()
            );
            assert!(
                snapshot
                    .outcome(&IdempotencyKeyV1::new("reset").unwrap())?
                    .is_some()
            );
            assert!(
                snapshot
                    .outcome(&IdempotencyKeyV1::new("ineligible").unwrap())?
                    .is_none()
            );
            assert_eq!(snapshot.committed_sequence()?, 4);
            Ok(())
        })
        .unwrap();
}

fn seed_graph(store: &DirectStoreV1, graph: &podway_store::GraphSessionStateV2) {
    let request = DirectMutationRequestV1::new(
        IdempotencyKeyV1::new("seed-graph").unwrap(),
        digest(),
        "workspace.repair",
        ORIGINAL_RESPONSE,
        DirectOutcomeScopeV1::Workspace,
    )
    .unwrap();
    store
        .mutate(
            &request,
            deadline(),
            || UnixMillis::new(NOW + 5),
            |transaction| {
                transaction.now();
                transaction.create_graph(graph)?;
                Ok(CanonicalExecutionJsonV1::new(canonicalize_json_v1(&json!({
            "schema":"podway.output/v4","request_id":ORIGINAL_RESPONSE,"command":"workspace.repair",
            "generated_at":"2023-11-14T22:13:20.005Z","result":{},"warnings":[]
        })).unwrap()).unwrap())
            },
        )
        .unwrap();
}

pub(super) fn deadline() -> Instant {
    Instant::now() + Duration::from_secs(10)
}
pub(super) fn digest() -> Sha256Digest {
    Sha256Digest::new(format!("sha256:{}", "a".repeat(64))).unwrap()
}

pub(super) fn fixture() -> (TempDir, WorktreeSelectorV1, DirectStoreV1, OutputEnvelopeV4) {
    let fixture = tempfile::Builder::new()
        .prefix("v2dfc006-engine-")
        .tempdir()
        .unwrap();
    assert!(
        Command::new("git")
            .args(["init", "--quiet"])
            .arg(fixture.path())
            .status()
            .unwrap()
            .success()
    );
    let path = std::fs::canonicalize(fixture.path()).unwrap();
    let selector = WorktreeSelectorV1::new(
        WORKTREE_SELECTOR_VERSION_V1,
        None,
        LosslessPathV1::from_raw_bytes(
            path.as_os_str().as_bytes(),
            DiagnosticPathDisplayV1::new(path.to_string_lossy()).unwrap(),
        )
        .unwrap(),
    )
    .unwrap();
    let worktree = NativeGitResolverV1::new()
        .resolve(selector.clone())
        .unwrap();
    WorkspaceLayoutInitializerV1::new()
        .initialize(&worktree)
        .unwrap();
    let root =
        ValidatedWorkspaceRootV1::from_unix_bytes(path.as_os_str().as_bytes().to_vec()).unwrap();
    let store = DirectStoreV1::new(
        DirectWorkspaceLayoutV1::new(root).unwrap(),
        digest(),
        worktree.identity().common_directory_fingerprint().clone(),
        worktree
            .identity()
            .worktree_administration_fingerprint()
            .clone(),
    );
    let output = OutputEnvelopeV4::from_value(json!({
        "schema":"podway.output/v4", "request_id":ORIGINAL_RESPONSE, "command":"workspace.init",
        "generated_at":"2023-11-14T22:13:20.000Z", "workspace": {
            "uuid":WORKSPACE, "root": path.to_string_lossy(), "latest_workspace_sequence":1
        }, "result":{"schema":"podway.workspace-init-result/v2","initialized":true,"revision":0},
        "warnings":[]
    }))
    .unwrap();
    let request = DirectMutationRequestV1::new(
        IdempotencyKeyV1::new("init-key").unwrap(),
        digest(),
        "workspace.init",
        ORIGINAL_RESPONSE,
        DirectOutcomeScopeV1::Workspace,
    )
    .unwrap();
    let lease = ExclusiveMaintenanceLeaseV1::acquire(store.layout(), deadline(), true).unwrap();
    store
        .bootstrap(
            &lease,
            WorkspaceId::new(WORKSPACE).unwrap(),
            "engine-fixture",
            &request,
            deadline(),
            || UnixMillis::new(NOW),
            |_| {
                Ok(
                    CanonicalExecutionJsonV1::new(canonicalize_json_v1(output.as_value()).unwrap())
                        .unwrap(),
                )
            },
        )
        .unwrap();
    drop(lease);
    (fixture, selector, store, output)
}

pub(super) struct Clock;
impl ExecutionClockV1 for Clock {
    fn now(&self) -> UnixMillis {
        UnixMillis::new(NOW + 1)
    }
}

fn request(selector: WorktreeSelectorV1, key: &str) -> OutcomeLookupRequestV1 {
    OutcomeLookupRequestV1 {
        selector,
        expected_workspace_id: Some(WorkspaceId::new(WORKSPACE).unwrap()),
        request_id: RequestIdV1::new(LOOKUP_RESPONSE).unwrap(),
        key: IdempotencyKeyV1::new(key).unwrap(),
        deadline: deadline(),
    }
}

#[test]
fn outcome_lookup_preserves_the_original_response_and_has_no_write_effect() {
    let (_fixture, selector, store, original) = fixture();
    let resolver = DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest());
    let before = std::fs::read(store.layout().database()).unwrap();
    let output =
        lookup_outcome_v1(&resolver, request(selector.clone(), "init-key"), &Clock).unwrap();
    assert!(validate_daemon_free_document_v1(
        "podway.output/v4",
        output.as_value()
    ));
    assert_eq!(output.as_value()["request_id"], LOOKUP_RESPONSE);
    assert_eq!(
        output.as_value()["result"]["outcome"]["response"],
        *original.as_value()
    );
    assert_eq!(
        output.as_value()["workspace"]["latest_workspace_sequence"],
        1
    );
    let missing = lookup_outcome_v1(&resolver, request(selector, "missing"), &Clock).unwrap();
    assert_eq!(missing.as_value()["result"]["found"], false);
    assert!(missing.as_value()["result"]["outcome"].is_null());
    assert_eq!(std::fs::read(store.layout().database()).unwrap(), before);
    assert_eq!(
        store
            .read(deadline(), |snapshot| snapshot.committed_sequence())
            .unwrap(),
        1
    );
}

#[test]
fn direct_resolution_rejects_a_replaced_workspace_uuid_before_reading_the_clock() {
    struct NoClock;
    impl ExecutionClockV1 for NoClock {
        fn now(&self) -> UnixMillis {
            panic!("rejected lookup read clock")
        }
    }
    let (_fixture, selector, _store, _original) = fixture();
    let resolver = DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest());
    let mut input = request(selector, "init-key");
    input.expected_workspace_id = Some(WorkspaceId::new(LOOKUP_RESPONSE).unwrap());
    assert!(matches!(
        lookup_outcome_v1(&resolver, input, &NoClock),
        Err(DirectExecutionErrorV1::Workspace(
            DirectWorkspaceResolutionErrorV1::Resolution(
                WorkspaceResolutionErrorV1::ExpectedWorkspaceUuidMismatch { .. }
            )
        ))
    ));
}

#[test]
fn direct_projection_families_validate_prepared_running_and_terminal_snapshots() {
    use podway_core::{AttemptId, ProcedureSnapshotId, ReasonV2, Revision, SessionId};
    use podway_engine::{
        direct_read_service::{
            project_direct_next_v1, project_direct_observation_v1, project_direct_status_v1,
        },
        execution::workspace_procedure_snapshot_from_bytes_v2,
        v2_read_service::GraphStatusTierV2,
    };
    use podway_store::GraphSessionStateV2;
    let delegated = delegated_procedure();
    for (source, path) in [
        (SIMPLE, "procedure.json"),
        (delegated.as_slice(), "delegated.json"),
    ] {
        for lifecycle in ["prepared", "running", "cancelled"] {
            let (_fixture, selector, store, _original) = fixture();
            let snapshot = workspace_procedure_snapshot_from_bytes_v2(
                path,
                source,
                ProcedureSnapshotId::new(LOOKUP_RESPONSE).unwrap(),
                UnixMillis::new(NOW + 2),
            )
            .unwrap();
            let mut graph = GraphSessionStateV2::prepared(
                Revision::new(1),
                "Direct read fixture",
                snapshot,
                SessionId::new(LOOKUP_RESPONSE).unwrap(),
                UnixMillis::new(NOW + 2),
            )
            .unwrap();
            if lifecycle != "prepared" {
                graph = graph
                    .begin_v2(
                        Revision::ZERO,
                        AttemptId::new(LOOKUP_RESPONSE).unwrap(),
                        None,
                        UnixMillis::new(NOW + 3),
                    )
                    .unwrap()
                    .into_state();
            }
            if lifecycle == "cancelled" {
                let attempt = graph.trace().active_attempt().unwrap().attempt_id().clone();
                graph = graph
                    .cancel_active_session_v2(
                        graph.trace().revision(),
                        &attempt,
                        ReasonV2::new("Exercise terminal projection.").unwrap(),
                        UnixMillis::new(NOW + 4),
                    )
                    .unwrap()
                    .into_state();
            }
            seed_graph(&store, &graph);
            store
                .read(deadline(), |snapshot| {
                    let view = snapshot.view(UnixMillis::new(NOW + 6))?;
                    for tier in [
                        GraphStatusTierV2::Compact,
                        GraphStatusTierV2::Standard,
                        GraphStatusTierV2::Verbose,
                    ] {
                        let status = project_direct_status_v1(&view, tier, None).unwrap();
                        assert!(!status.contains_key("queue"));
                    }
                    let observation = project_direct_observation_v1(&view).unwrap();
                    assert!(observation["status"].get("queue").is_none());
                    assert!(observation["guidance"].get("queue").is_none());
                    if lifecycle == "cancelled" {
                        assert!(observation["guidance"].is_null());
                        assert!(project_direct_next_v1(&view).is_err());
                    } else {
                        let next = project_direct_next_v1(&view).unwrap();
                        assert!(!next.contains_key("queue"));
                        assert_eq!(
                            next["schema"],
                            if lifecycle == "prepared" {
                                "podway.prepared-next-result/v3"
                            } else {
                                "podway.next-result/v5"
                            }
                        );
                    }
                    Ok(())
                })
                .unwrap();
            use podway_engine::direct_execution::{
                DirectReadCommandV1, DirectReadRequestV1, execute_read_v1,
            };
            let resolver = DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest());
            for command in [
                DirectReadCommandV1::Status {
                    tier: GraphStatusTierV2::Standard,
                    history_before: None,
                },
                DirectReadCommandV1::Observe,
            ] {
                let output = execute_read_v1(
                    &resolver,
                    DirectReadRequestV1 {
                        request_id: RequestIdV1::new(LOOKUP_RESPONSE).unwrap(),
                        selector: selector.clone(),
                        expected_workspace_id: Some(WorkspaceId::new(WORKSPACE).unwrap()),
                        expected_session_id: Some(SessionId::new(LOOKUP_RESPONSE).unwrap()),
                        command,
                        deadline: deadline(),
                    },
                    &Clock,
                )
                .unwrap();
                assert!(validate_daemon_free_document_v1(
                    "podway.output/v4",
                    output.as_value()
                ));
                assert_eq!(
                    output.as_value()["workspace"]["latest_workspace_sequence"],
                    2
                );
            }
            let mismatch = execute_read_v1(
                &resolver,
                DirectReadRequestV1 {
                    request_id: RequestIdV1::new(LOOKUP_RESPONSE).unwrap(),
                    selector,
                    expected_workspace_id: Some(WorkspaceId::new(WORKSPACE).unwrap()),
                    expected_session_id: Some(SessionId::new(ORIGINAL_RESPONSE).unwrap()),
                    command: DirectReadCommandV1::Observe,
                    deadline: deadline(),
                },
                &Clock,
            );
            assert!(matches!(
                mismatch,
                Err(DirectExecutionErrorV1::Domain(
                    podway_core::DomainError::SessionIdentityMismatch { .. }
                ))
            ));
        }
    }
}

#[test]
fn begin_replays_original_success_before_stale_fences_clock_or_identifiers() {
    use podway_core::{
        AttemptId, BlockerId, DelegationReceiptIdV1, ProcedureSnapshotId, Revision, SessionId,
    };
    use podway_engine::{
        direct_execution::{DirectBeginRequestV1, DirectIdSourceV1, begin_session_v1},
        execution::workspace_procedure_snapshot_from_bytes_v2,
    };
    use podway_protocol::{SessionBeginV1, SessionIdentityPreconditionsWireV1};
    use podway_store::{
        GraphMutationErrorV2, GraphSessionStateV2, direct_store::DirectStoreErrorV1,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct CountClock(AtomicUsize);
    impl ExecutionClockV1 for CountClock {
        fn now(&self) -> UnixMillis {
            assert_eq!(
                self.0.fetch_add(1, Ordering::SeqCst),
                0,
                "mutation clock read more than once"
            );
            UnixMillis::new(NOW + 6)
        }
    }
    struct Ids(AtomicUsize);
    impl DirectIdSourceV1 for Ids {
        fn next_workspace_id(&self) -> WorkspaceId {
            panic!("unexpected workspace ID")
        }
        fn next_session_id(&self) -> SessionId {
            panic!("unexpected session ID")
        }
        fn next_attempt_id(&self) -> AttemptId {
            assert_eq!(
                self.0.fetch_add(1, Ordering::SeqCst),
                0,
                "replay generated an attempt"
            );
            AttemptId::new(ORIGINAL_RESPONSE).unwrap()
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
    let (_fixture, selector, store, _original) = fixture();
    let snapshot = workspace_procedure_snapshot_from_bytes_v2(
        "procedure.json",
        SIMPLE,
        ProcedureSnapshotId::new(LOOKUP_RESPONSE).unwrap(),
        UnixMillis::new(NOW + 2),
    )
    .unwrap();
    let graph = GraphSessionStateV2::prepared(
        Revision::new(1),
        "Begin fixture",
        snapshot,
        SessionId::new(LOOKUP_RESPONSE).unwrap(),
        UnixMillis::new(NOW + 2),
    )
    .unwrap();
    seed_graph(&store, &graph);
    let resolver = DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest());
    let clock = CountClock(AtomicUsize::new(0));
    let ids = Ids(AtomicUsize::new(0));
    let input = DirectBeginRequestV1 {
        request_id: RequestIdV1::new(ORIGINAL_RESPONSE).unwrap(),
        selector,
        expected_workspace_id: Some(WorkspaceId::new(WORKSPACE).unwrap()),
        key: IdempotencyKeyV1::new("begin-key").unwrap(),
        input: SessionBeginV1 {
            initial_goal: None,
            preconditions: SessionIdentityPreconditionsWireV1 {
                expected_session_id: graph.trace().session_id().clone(),
                expected_session_revision: Revision::ZERO,
            },
        },
        deadline: deadline(),
    };
    let first = begin_session_v1(&resolver, input.clone(), &clock, &ids).unwrap();
    assert!(validate_daemon_free_document_v1(
        "podway.output/v4",
        first.as_value()
    ));
    assert_eq!(
        first.as_value()["workspace"]["latest_workspace_sequence"],
        3
    );
    assert_eq!(first.as_value()["result"]["revision"], 1);
    let mut replay = input.clone();
    replay.request_id = RequestIdV1::new(LOOKUP_RESPONSE).unwrap();
    let second = begin_session_v1(&resolver, replay, &clock, &ids).unwrap();
    assert_eq!(first, second);
    let mut reused = input.clone();
    reused.input.preconditions.expected_session_revision = Revision::new(1);
    assert!(matches!(
        begin_session_v1(&resolver, reused, &clock, &ids),
        Err(DirectExecutionErrorV1::Store(
            DirectStoreErrorV1::IdempotencyKeyReused
        ))
    ));
    let mut stale = input;
    stale.key = IdempotencyKeyV1::new("stale-key").unwrap();
    assert!(matches!(
        begin_session_v1(&resolver, stale, &clock, &ids),
        Err(DirectExecutionErrorV1::Store(DirectStoreErrorV1::Domain(
            GraphMutationErrorV2::SessionRevisionConflict { .. }
        )))
    ));
    assert_eq!(clock.0.load(Ordering::SeqCst), 1);
    assert_eq!(ids.0.load(Ordering::SeqCst), 1);
    store
        .read(deadline(), |snapshot| {
            assert_eq!(snapshot.committed_sequence()?, 3);
            assert!(
                snapshot
                    .outcome(&IdempotencyKeyV1::new("stale-key").unwrap())?
                    .is_none()
            );
            let graph = snapshot.current_graph()?.unwrap();
            assert_eq!(graph.trace().revision(), Revision::new(1));
            assert_eq!(graph.trace().attempts().len(), 1);
            Ok(())
        })
        .unwrap();
}

#[test]
fn cursor_commands_preserve_fences_history_and_exact_replay_across_a_real_store() {
    use podway_core::{
        AttemptId, BlockerId, DelegationReceiptIdV1, ProcedureSnapshotId, Revision, SessionId,
    };
    use podway_engine::{
        direct_execution::{
            DirectCursorCommandV1, DirectCursorRequestV1, DirectIdSourceV1, NativeDirectIdSourceV1,
            execute_cursor_v1,
        },
        execution::workspace_procedure_snapshot_from_bytes_v2,
    };
    use podway_protocol::{
        SessionBlockV1, SessionCancelV1, SessionMutationPreconditionsWireV1, SessionRetryV1,
        SessionUnblockV1,
    };
    use podway_store::GraphSessionStateV2;
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct CountClock(AtomicUsize);
    impl ExecutionClockV1 for CountClock {
        fn now(&self) -> UnixMillis {
            UnixMillis::new(NOW + 20 + self.0.fetch_add(1, Ordering::SeqCst) as u64)
        }
    }
    struct Ids {
        attempts: AtomicUsize,
        blockers: AtomicUsize,
    }
    impl DirectIdSourceV1 for Ids {
        fn next_workspace_id(&self) -> WorkspaceId {
            panic!("unexpected workspace ID")
        }
        fn next_session_id(&self) -> SessionId {
            panic!("unexpected session ID")
        }
        fn next_attempt_id(&self) -> AttemptId {
            self.attempts.fetch_add(1, Ordering::SeqCst);
            NativeDirectIdSourceV1.next_attempt_id()
        }
        fn next_blocker_id(&self) -> BlockerId {
            self.blockers.fetch_add(1, Ordering::SeqCst);
            NativeDirectIdSourceV1.next_blocker_id()
        }
        fn next_procedure_snapshot_id(&self) -> ProcedureSnapshotId {
            panic!("unexpected snapshot ID")
        }
        fn next_delegation_receipt_id(&self) -> DelegationReceiptIdV1 {
            panic!("unexpected receipt ID")
        }
    }
    let (_fixture, selector, store, _original) = fixture();
    let snapshot = workspace_procedure_snapshot_from_bytes_v2(
        "procedure.json",
        SIMPLE,
        ProcedureSnapshotId::new(LOOKUP_RESPONSE).unwrap(),
        UnixMillis::new(NOW + 2),
    )
    .unwrap();
    let graph = GraphSessionStateV2::prepared(
        Revision::new(1),
        "Cursor fixture",
        snapshot,
        SessionId::new(LOOKUP_RESPONSE).unwrap(),
        UnixMillis::new(NOW + 2),
    )
    .unwrap()
    .begin_v2(
        Revision::ZERO,
        AttemptId::new(LOOKUP_RESPONSE).unwrap(),
        None,
        UnixMillis::new(NOW + 3),
    )
    .unwrap()
    .into_state();
    seed_graph(&store, &graph);
    let resolver = DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest());
    let clock = CountClock(AtomicUsize::new(0));
    let ids = Ids {
        attempts: AtomicUsize::new(0),
        blockers: AtomicUsize::new(0),
    };
    let fences = |revision, attempt: &AttemptId| SessionMutationPreconditionsWireV1 {
        expected_session_id: graph.trace().session_id().clone(),
        expected_session_revision: Revision::new(revision),
        expected_attempt_id: attempt.clone(),
    };
    let run = |command: DirectCursorCommandV1, key: &str, expected_sequence| {
        let input = DirectCursorRequestV1 {
            request_id: RequestIdV1::new(ORIGINAL_RESPONSE).unwrap(),
            selector: selector.clone(),
            expected_workspace_id: Some(WorkspaceId::new(WORKSPACE).unwrap()),
            key: IdempotencyKeyV1::new(key).unwrap(),
            command,
            deadline: deadline(),
        };
        let first = execute_cursor_v1(&resolver, input.clone(), &clock, &ids).unwrap();
        assert!(validate_daemon_free_document_v1(
            "podway.output/v4",
            first.as_value()
        ));
        assert_eq!(
            first.as_value()["workspace"]["latest_workspace_sequence"],
            expected_sequence
        );
        let counts = (
            clock.0.load(Ordering::SeqCst),
            ids.attempts.load(Ordering::SeqCst),
            ids.blockers.load(Ordering::SeqCst),
        );
        let mut replay = input;
        replay.request_id = RequestIdV1::new(LOOKUP_RESPONSE).unwrap();
        assert_eq!(
            execute_cursor_v1(&resolver, replay, &clock, &ids).unwrap(),
            first
        );
        assert_eq!(
            (
                clock.0.load(Ordering::SeqCst),
                ids.attempts.load(Ordering::SeqCst),
                ids.blockers.load(Ordering::SeqCst)
            ),
            counts
        );
        first
    };
    let initial = AttemptId::new(LOOKUP_RESPONSE).unwrap();
    let blocked = run(
        DirectCursorCommandV1::Block(SessionBlockV1 {
            reason: "Await fixture input.".into(),
            preconditions: fences(1, &initial),
        }),
        "block-one",
        3,
    );
    let blocker =
        BlockerId::new(blocked.as_value()["result"]["blocker_id"].as_str().unwrap()).unwrap();
    run(
        DirectCursorCommandV1::Unblock(SessionUnblockV1 {
            blocker_id: Some(blocker),
            all: false,
            preconditions: fences(2, &initial),
        }),
        "unblock-one",
        4,
    );
    run(
        DirectCursorCommandV1::Block(SessionBlockV1 {
            reason: "Await another fixture input.".into(),
            preconditions: fences(3, &initial),
        }),
        "block-two",
        5,
    );
    run(
        DirectCursorCommandV1::Unblock(SessionUnblockV1 {
            blocker_id: None,
            all: true,
            preconditions: fences(4, &initial),
        }),
        "unblock-all",
        6,
    );
    let retried = run(
        DirectCursorCommandV1::Retry(SessionRetryV1 {
            reason: "Retry the fixture work.".into(),
            preconditions: fences(5, &initial),
        }),
        "retry",
        7,
    );
    let next = AttemptId::new(
        retried.as_value()["result"]["to_attempt_id"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    assert_ne!(next, initial);
    run(
        DirectCursorCommandV1::Cancel(SessionCancelV1 {
            reason: "End the fixture work.".into(),
            preconditions: fences(6, &next),
        }),
        "cancel",
        8,
    );
    assert_eq!(clock.0.load(Ordering::SeqCst), 6);
    assert_eq!(ids.attempts.load(Ordering::SeqCst), 1);
    assert_eq!(ids.blockers.load(Ordering::SeqCst), 2);
    store
        .read(deadline(), |snapshot| {
            let graph = snapshot.current_graph()?.unwrap();
            assert_eq!(snapshot.committed_sequence()?, 8);
            assert_eq!(graph.trace().revision(), Revision::new(7));
            assert_eq!(graph.trace().attempts().len(), 2);
            assert_eq!(graph.cancel_reason().unwrap(), "End the fixture work.");
            Ok(())
        })
        .unwrap();
}

#[test]
fn direct_artifact_verification_checks_prepared_stamps_without_artifact_byte_reads() {
    use podway_core::DomainError;
    use podway_engine::{
        execution::ExecutionBoundaryErrorV1, native_execution::NativeDirectWorkspaceV1,
    };
    use podway_store::{GraphMutationErrorV2, direct_store::DirectStoreErrorV1};
    use std::sync::atomic::{AtomicUsize, Ordering};
    let (fixture, selector, store, _original) = fixture();
    std::fs::write(fixture.path().join("report.txt"), "Original report.\n").unwrap();
    let resolver = DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest());
    let (resolved, _) = resolver
        .resolve_existing(
            selector,
            Some(&WorkspaceId::new(WORKSPACE).unwrap()),
            deadline(),
        )
        .unwrap();
    let verifier = NativeDirectWorkspaceV1::new(resolved);
    let binding = store
        .read(deadline(), |snapshot| Ok(snapshot.binding().clone()))
        .unwrap();
    let prepared = verifier
        .prepare_local_artifact_v1(&binding, "report.txt", Some("text/plain"))
        .unwrap();

    let clock_reads = AtomicUsize::new(0);
    let request = DirectMutationRequestV1::new(
        IdempotencyKeyV1::new("verify-artifact").unwrap(),
        digest(),
        "workspace.repair",
        ORIGINAL_RESPONSE,
        DirectOutcomeScopeV1::Workspace,
    )
    .unwrap();
    store.mutate(&request, deadline(), || { clock_reads.fetch_add(1, Ordering::SeqCst); UnixMillis::new(NOW + 20) },
        |transaction| {
            verifier.verify_prepared_local_artifact_v1(transaction.binding(), &prepared).unwrap();
            transaction.now();
            Ok(CanonicalExecutionJsonV1::new(canonicalize_json_v1(&json!({
                "schema":"podway.output/v4", "request_id":ORIGINAL_RESPONSE, "command":"workspace.repair",
                "generated_at":"2023-11-14T22:13:20.020Z", "result":{}, "warnings":[]
            })).unwrap()).unwrap())
        }).unwrap();
    assert_eq!(clock_reads.load(Ordering::SeqCst), 1);
    std::fs::write(fixture.path().join("report.txt"), "Changed report.\n").unwrap();
    let changed = DirectMutationRequestV1::new(
        IdempotencyKeyV1::new("changed-artifact").unwrap(),
        digest(),
        "workspace.repair",
        ORIGINAL_RESPONSE,
        DirectOutcomeScopeV1::Workspace,
    )
    .unwrap();
    let result = store.mutate(
        &changed,
        deadline(),
        || panic!("changed artifact read the mutation clock"),
        |transaction| match verifier
            .verify_prepared_local_artifact_v1(transaction.binding(), &prepared)
        {
            Err(ExecutionBoundaryErrorV1::Domain(DomainError::ArtifactChanged)) => {
                Err(GraphMutationErrorV2::Domain(DomainError::ArtifactChanged).into())
            }
            other => panic!("unexpected artifact revalidation: {other:?}"),
        },
    );
    assert!(matches!(
        result,
        Err(DirectStoreErrorV1::Domain(GraphMutationErrorV2::Domain(
            DomainError::ArtifactChanged
        )))
    ));
    store
        .read(deadline(), |snapshot| {
            assert_eq!(snapshot.committed_sequence()?, 2);
            assert!(
                snapshot
                    .outcome(&IdempotencyKeyV1::new("changed-artifact").unwrap())?
                    .is_none()
            );
            Ok(())
        })
        .unwrap();
    std::os::unix::fs::symlink(
        fixture.path().join("report.txt"),
        fixture.path().join("report-link.txt"),
    )
    .unwrap();
    assert!(
        verifier
            .prepare_local_artifact_v1(&binding, "report-link.txt", None)
            .is_err()
    );
    assert!(
        verifier
            .prepare_local_artifact_v1(&binding, "../report.txt", None)
            .is_err()
    );
}
