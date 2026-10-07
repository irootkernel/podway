//! Real Git worktrees and generation-14 workspace commands without a daemon.
use podway_core::{
    AttemptId, BlockerId, DelegationReceiptIdV1, ProcedureSnapshotId, SessionId, Sha256Digest,
    UnixMillis, WorkspaceId,
};
use podway_engine::{
    direct_execution::{DirectIdSourceV1, NativeDirectIdSourceV1},
    direct_workspace::{
        DirectWorkspaceErrorV1, DirectWorkspaceMutationRequestV1, DirectWorkspaceReadRequestV1,
        doctor_workspace_v1, initialize_workspace_v1, repair_workspace_v1, show_workspace_v1,
    },
    execution::ExecutionClockV1,
    workspace::DirectWorkspaceResolverV1,
};
use podway_git::{
    DiagnosticPathDisplayV1, LosslessPathV1, NativeGitResolverV1, WORKTREE_SELECTOR_VERSION_V1,
    WorktreeSelectorV1,
};
use podway_protocol::{RequestIdV1, validate_daemon_free_document_v1};
use podway_store::{IdempotencyKeyV1, direct_store::DirectStoreErrorV1};
use sha2::{Digest as _, Sha256};
use std::{
    collections::BTreeMap,
    os::unix::ffi::OsStrExt,
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
    time::{Duration, Instant},
};
use tempfile::TempDir;
const RESPONSE: &str = "00000000-0000-4000-8000-000000000001";
const DIFFERENT: &str = "00000000-0000-4000-8000-000000000002";

#[test]
fn bounded_preliminary_retries_preserve_native_static_layout_and_durable_binding_failures() {
    use podway_engine::workspace::{DirectWorkspaceResolutionErrorV1, WorkspaceResolutionErrorV1};
    use podway_git::{
        DurableWorktreeIdentityV1, GitRepresentationProblemV1, GitResolveErrorV1,
        GitResolverContractV1, ValidatedWorktreeV1,
    };
    struct CountedNative(std::sync::Arc<AtomicUsize>);
    impl GitResolverContractV1 for CountedNative {
        fn resolve(
            &self,
            selector: WorktreeSelectorV1,
        ) -> Result<ValidatedWorktreeV1, GitResolveErrorV1> {
            self.0.fetch_add(1, Ordering::SeqCst);
            NativeGitResolverV1::new().resolve(selector)
        }
    }
    let unsafe_root = fixture();
    std::fs::write(unsafe_root.path().join(".podway"), b"ordinary file").unwrap();
    let calls = std::sync::Arc::new(AtomicUsize::new(0));
    let counted = DirectWorkspaceResolverV1::new(CountedNative(calls.clone()), digest());
    for existing in [false, true] {
        let error = if existing {
            counted
                .resolve_existing(selector(unsafe_root.path()), None, deadline())
                .unwrap_err()
        } else {
            counted
                .resolve_candidate(selector(unsafe_root.path()))
                .unwrap_err()
        };
        assert!(matches!(
            error,
            DirectWorkspaceResolutionErrorV1::Resolution(WorkspaceResolutionErrorV1::Git {
                source: GitResolveErrorV1::Representation {
                    problem: GitRepresentationProblemV1::UnsupportedRepositoryLayout
                },
                ..
            })
        ));
    }
    assert_eq!(calls.load(Ordering::SeqCst), 16);
    assert_eq!(
        std::fs::read(unsafe_root.path().join(".podway")).unwrap(),
        b"ordinary file"
    );
    let root = fixture();
    let resolver = DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest());
    initialize_workspace_v1(
        &resolver,
        mutation(root.path(), "init"),
        false,
        &Clock(AtomicUsize::new(0)),
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    let (resolved, _) = resolver
        .resolve_existing(selector(root.path()), None, deadline())
        .unwrap();
    let identity = resolved.worktree().identity();
    let wrong = WorkspaceId::new(DIFFERENT).unwrap();
    let fenced = WorktreeSelectorV1::new(
        WORKTREE_SELECTOR_VERSION_V1,
        Some(
            DurableWorktreeIdentityV1::new_with_root_directory_fingerprint(
                wrong.clone(),
                identity.common_directory_fingerprint().clone(),
                identity.worktree_administration_fingerprint().clone(),
                identity.root_directory_fingerprint().unwrap().clone(),
                identity.last_validated_root().clone(),
            ),
        ),
        selector(root.path()).path().clone(),
    )
    .unwrap();
    let before = files(&root.path().join(".podway"));
    let error = show_workspace_v1(
        &resolver,
        DirectWorkspaceReadRequestV1 {
            selector: fenced,
            expected_workspace_id: None,
            request_id: RequestIdV1::new(RESPONSE).unwrap(),
            deadline: deadline(),
        },
        &NoEffects,
    )
    .unwrap_err();
    assert!(
        matches!(error,DirectWorkspaceErrorV1::Resolution(DirectWorkspaceResolutionErrorV1::Resolution(
        WorkspaceResolutionErrorV1::ExpectedWorkspaceUuidMismatch {expected,..})) if expected==wrong)
    );
    assert_eq!(files(&root.path().join(".podway")), before);
}
struct Clock(AtomicUsize);
impl ExecutionClockV1 for Clock {
    fn now(&self) -> UnixMillis {
        self.0.fetch_add(1, Ordering::SeqCst);
        UnixMillis::new(1_700_000_000_000)
    }
}
struct NoEffects;
impl ExecutionClockV1 for NoEffects {
    fn now(&self) -> UnixMillis {
        panic!("rejection/replay read clock")
    }
}
impl DirectIdSourceV1 for NoEffects {
    fn next_workspace_id(&self) -> WorkspaceId {
        panic!("rejection/replay generated UUID")
    }
    fn next_session_id(&self) -> SessionId {
        panic!("unexpected session UUID")
    }
    fn next_attempt_id(&self) -> AttemptId {
        panic!("unexpected attempt UUID")
    }
    fn next_blocker_id(&self) -> BlockerId {
        panic!("unexpected blocker UUID")
    }
    fn next_procedure_snapshot_id(&self) -> ProcedureSnapshotId {
        panic!("unexpected snapshot UUID")
    }
    fn next_delegation_receipt_id(&self) -> DelegationReceiptIdV1 {
        panic!("unexpected receipt UUID")
    }
}
fn deadline() -> Instant {
    Instant::now() + Duration::from_secs(10)
}
fn digest() -> Sha256Digest {
    Sha256Digest::new(format!("sha256:{}", "a".repeat(64))).unwrap()
}
fn selector(path: &std::path::Path) -> WorktreeSelectorV1 {
    WorktreeSelectorV1::new(
        WORKTREE_SELECTOR_VERSION_V1,
        None,
        LosslessPathV1::from_raw_bytes(
            path.as_os_str().as_bytes().to_vec(),
            DiagnosticPathDisplayV1::new(path.to_string_lossy()).unwrap(),
        )
        .unwrap(),
    )
    .unwrap()
}
fn fixture() -> TempDir {
    let root = tempfile::tempdir().unwrap();
    assert!(
        Command::new("git")
            .args(["init", "--quiet"])
            .arg(root.path())
            .status()
            .unwrap()
            .success()
    );
    root
}
fn mutation(path: &std::path::Path, key: &str) -> DirectWorkspaceMutationRequestV1 {
    DirectWorkspaceMutationRequestV1 {
        selector: selector(path),
        expected_workspace_id: None,
        request_id: RequestIdV1::new(RESPONSE).unwrap(),
        key: IdempotencyKeyV1::new(key).unwrap(),
        deadline: deadline(),
    }
}
fn read(path: &std::path::Path) -> DirectWorkspaceReadRequestV1 {
    DirectWorkspaceReadRequestV1 {
        selector: selector(path),
        expected_workspace_id: None,
        request_id: RequestIdV1::new(DIFFERENT).unwrap(),
        deadline: deadline(),
    }
}
fn files(root: &std::path::Path) -> BTreeMap<String, String> {
    fn visit(
        root: &std::path::Path,
        path: &std::path::Path,
        result: &mut BTreeMap<String, String>,
    ) {
        for entry in std::fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                visit(root, &path, result);
            } else {
                result.insert(
                    path.strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                    format!("{:x}", Sha256::digest(std::fs::read(path).unwrap())),
                );
            }
        }
    }
    let mut result = BTreeMap::new();
    visit(root, root, &mut result);
    result
}
#[test]
fn init_show_doctor_and_repair_validate_and_replay_without_runtime_or_read_writes() {
    let root = fixture();
    let resolver = DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest());
    let clock = Clock(AtomicUsize::new(0));
    assert!(matches!(
        show_workspace_v1(&resolver, read(root.path()), &NoEffects),
        Err(DirectWorkspaceErrorV1::Resolution(_))
    ));
    let initial = initialize_workspace_v1(
        &resolver,
        mutation(root.path(), "init"),
        false,
        &clock,
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    assert!(validate_daemon_free_document_v1(
        "podway.output/v4",
        initial.as_value()
    ));
    assert_eq!(
        initial.as_value()["workspace"]["latest_workspace_sequence"],
        1
    );
    assert_eq!(clock.0.load(Ordering::SeqCst), 1);
    assert!(
        !root
            .path()
            .join(".podway/runtime/maintenance-intent.json")
            .exists()
    );
    assert!(
        !std::fs::read_to_string(root.path().join(".podway/config.yaml"))
            .unwrap()
            .contains("job_queue")
    );
    let mut replay = mutation(root.path(), "init");
    replay.request_id = RequestIdV1::new(DIFFERENT).unwrap();
    assert_eq!(
        initialize_workspace_v1(&resolver, replay, false, &NoEffects, &NoEffects).unwrap(),
        initial
    );
    assert!(matches!(
        initialize_workspace_v1(
            &resolver,
            mutation(root.path(), "init"),
            true,
            &NoEffects,
            &NoEffects
        ),
        Err(DirectWorkspaceErrorV1::Store(
            DirectStoreErrorV1::IdempotencyKeyReused
        ))
    ));
    let before = files(&root.path().join(".podway"));
    let show = show_workspace_v1(&resolver, read(root.path()), &clock).unwrap();
    assert_eq!(show.as_value()["result"]["root_rebind_required"], false);
    for deep in [false, true] {
        let result = doctor_workspace_v1(&resolver, read(root.path()), deep, &clock).unwrap();
        assert!(validate_daemon_free_document_v1(
            "podway.output/v4",
            result.as_value()
        ));
        assert_eq!(result.as_value()["result"]["healthy"], true);
    }
    assert_eq!(
        files(&root.path().join(".podway")),
        before,
        "read commands changed runtime bytes"
    );
    let repair = repair_workspace_v1(&resolver, mutation(root.path(), "repair"), &clock).unwrap();
    assert_eq!(repair.as_value()["result"]["changed"], false);
    assert_eq!(
        repair.as_value()["workspace"]["latest_workspace_sequence"],
        2
    );
    assert_eq!(
        repair_workspace_v1(&resolver, mutation(root.path(), "repair"), &NoEffects).unwrap(),
        repair
    );
    let mut fenced = read(root.path());
    fenced.expected_workspace_id = Some(WorkspaceId::new(DIFFERENT).unwrap());
    assert!(matches!(
        show_workspace_v1(&resolver, fenced, &NoEffects),
        Err(DirectWorkspaceErrorV1::Resolution(_))
    ));
    let foreign = DirectWorkspaceResolverV1::new(
        NativeGitResolverV1::new(),
        Sha256Digest::new(format!("sha256:{}", "b".repeat(64))).unwrap(),
    );
    assert!(matches!(
        doctor_workspace_v1(&foreign, read(root.path()), true, &NoEffects),
        Err(DirectWorkspaceErrorV1::Resolution(_))
    ));
}
#[test]
fn moved_root_is_reported_without_write_and_rebound_only_by_repair_commit() {
    let parent = fixture();
    let old = parent.path().join("old");
    assert!(
        Command::new("git")
            .args([
                "-c",
                "user.name=Fixture",
                "-c",
                "user.email=fixture@example.test",
                "commit",
                "--quiet",
                "--allow-empty",
                "-m",
                "Fixture"
            ])
            .current_dir(parent.path())
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("git")
            .args(["worktree", "add", "--quiet", "--detach"])
            .arg(&old)
            .arg("HEAD")
            .current_dir(parent.path())
            .status()
            .unwrap()
            .success()
    );
    let resolver = DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest());
    let clock = Clock(AtomicUsize::new(0));
    initialize_workspace_v1(
        &resolver,
        mutation(&old, "init"),
        false,
        &clock,
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    let new = parent.path().join("new");
    assert!(
        Command::new("git")
            .args(["worktree", "move"])
            .arg(&old)
            .arg(&new)
            .current_dir(parent.path())
            .status()
            .unwrap()
            .success()
    );
    let before = files(&new.join(".podway"));
    let show = show_workspace_v1(&resolver, read(&new), &clock).unwrap();
    assert_eq!(show.as_value()["result"]["root_rebind_required"], true);
    assert_eq!(files(&new.join(".podway")), before);
    let repair = repair_workspace_v1(&resolver, mutation(&new, "repair"), &clock).unwrap();
    assert_eq!(repair.as_value()["result"]["changed"], true);
    assert_eq!(
        show_workspace_v1(&resolver, read(&new), &clock)
            .unwrap()
            .as_value()["result"]["root_rebind_required"],
        false
    );
    assert_eq!(
        repair_workspace_v1(&resolver, mutation(&new, "repair"), &NoEffects).unwrap(),
        repair
    );
}

#[test]
fn init_crash_child() {
    use podway_engine::direct_workspace::initialize_workspace_with_boundary_observer_v1;
    use podway_store::direct_store::DirectTransactionBoundaryV1;
    let Some(path) = std::env::var_os("PODWAY_WORKSPACE_INIT_FIXTURE") else {
        return;
    };
    let phase = std::env::var("PODWAY_WORKSPACE_INIT_PHASE").unwrap();
    let resolver = DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest());
    initialize_workspace_with_boundary_observer_v1(
        &resolver,
        mutation(std::path::Path::new(&path), "crashed-init"),
        false,
        &Clock(AtomicUsize::new(0)),
        &NativeDirectIdSourceV1,
        move |boundary| {
            let selected = match phase.as_str() {
                "before_begin" => DirectTransactionBoundaryV1::BeforeBegin,
                "before_outcome" => DirectTransactionBoundaryV1::AfterStateBeforeOutcome,
                "before_publication" => DirectTransactionBoundaryV1::BeforeMaintenancePublication,
                "after_publication" => DirectTransactionBoundaryV1::AfterCommitBeforeOutput,
                _ => panic!("unknown owned crash phase"),
            };
            if boundary == selected {
                std::process::exit(77);
            }
        },
    )
    .unwrap();
    panic!("owned crash boundary was not observed");
}

#[test]
fn exact_init_retry_resumes_private_or_published_generation_and_other_requests_fail_closed() {
    use podway_engine::workspace::DirectWorkspaceResolutionErrorV1;
    for phase in [
        "before_begin",
        "before_outcome",
        "before_publication",
        "after_publication",
    ] {
        let root = fixture();
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "int_v2dfc006_workspace::init_crash_child",
                "--nocapture",
            ])
            .env("PODWAY_WORKSPACE_INIT_FIXTURE", root.path())
            .env("PODWAY_WORKSPACE_INIT_PHASE", phase)
            .status()
            .unwrap();
        assert_eq!(
            status.code(),
            Some(77),
            "{phase}: actual Engine crash child did not stop at boundary"
        );
        let resolver = DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest());
        assert!(
            matches!(
                show_workspace_v1(&resolver, read(root.path()), &NoEffects),
                Err(DirectWorkspaceErrorV1::Resolution(
                    DirectWorkspaceResolutionErrorV1::Store(DirectStoreErrorV1::Maintenance)
                ))
            ),
            "{phase}"
        );
        assert!(
            matches!(
                initialize_workspace_v1(
                    &resolver,
                    mutation(root.path(), "another-init"),
                    false,
                    &NoEffects,
                    &NoEffects
                ),
                Err(DirectWorkspaceErrorV1::Store(
                    DirectStoreErrorV1::Maintenance
                ))
            ),
            "{phase}"
        );
        let resumed = initialize_workspace_v1(
            &resolver,
            mutation(root.path(), "crashed-init"),
            false,
            &NoEffects,
            &NoEffects,
        )
        .unwrap();
        assert!(validate_daemon_free_document_v1(
            "podway.output/v4",
            resumed.as_value()
        ));
        assert_eq!(
            resumed.as_value()["workspace"]["latest_workspace_sequence"],
            1
        );
        assert_eq!(
            initialize_workspace_v1(
                &resolver,
                mutation(root.path(), "crashed-init"),
                false,
                &NoEffects,
                &NoEffects
            )
            .unwrap(),
            resumed
        );
        doctor_workspace_v1(
            &resolver,
            read(root.path()),
            true,
            &Clock(AtomicUsize::new(0)),
        )
        .unwrap();
    }
}

#[test]
fn deep_doctor_detects_changed_required_artifact_without_writing_or_reading_clock() {
    use podway_core::{ItemId, Revision};
    use podway_engine::{
        direct_execution::{DirectBeginRequestV1, begin_session_v1},
        direct_items::{DirectItemCommandV1, DirectItemRequestV1, execute_item_v1},
        direct_start::{DirectStartCommandV1, DirectStartRequestV1, start_session_v1},
        execution::prepare_workspace_procedure_source_v2,
    };
    use podway_protocol::{
        ItemAttachSourceV1, ItemAttachV1, ItemMutationPreconditionsWireV1, SessionBeginV1,
        SessionIdentityPreconditionsWireV1, SessionStartSourceV1, SessionStartV1,
    };
    let root = fixture();
    let resolver = DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest());
    let clock = Clock(AtomicUsize::new(0));
    initialize_workspace_v1(
        &resolver,
        mutation(root.path(), "init"),
        false,
        &clock,
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    let source = br#"{"schema":"podway.procedure/v2","id":"diagnostic","version":"1","name":"Diagnostic","purpose":"Check real artifact diagnostics.",
        "node_definitions":{"work":{"type":"action","title":"Work","intent":"Attach evidence.","items":[{"id":"report","type":"artifact","prompt":"Attach report.","required":true}]}},
        "graph":{"entry":"work","nodes":[{"id":"work","use":"work","terminal":true}]}}"#;
    std::fs::write(root.path().join("procedure.json"), source).unwrap();
    std::fs::write(root.path().join("report.txt"), "Original report").unwrap();
    let prepared = prepare_workspace_procedure_source_v2("procedure.json", source).unwrap();
    let start = start_session_v1(
        &resolver,
        DirectStartRequestV1 {
            selector: selector(root.path()),
            expected_workspace_id: None,
            request_id: RequestIdV1::new(RESPONSE).unwrap(),
            key: Some(IdempotencyKeyV1::new("start").unwrap()),
            deadline: deadline(),
            command: DirectStartCommandV1::Start(SessionStartV1 {
                source: SessionStartSourceV1::Procedure {
                    procedure: "procedure.json".into(),
                },
                expected_procedure_digest: Some(prepared.digest().clone()),
                task_title: "Diagnose current evidence".into(),
                dry_run: false,
            }),
        },
        &clock,
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    let session = SessionId::new(start.as_value()["session"]["id"].as_str().unwrap()).unwrap();
    let begin = begin_session_v1(
        &resolver,
        DirectBeginRequestV1 {
            selector: selector(root.path()),
            expected_workspace_id: None,
            request_id: RequestIdV1::new(RESPONSE).unwrap(),
            key: IdempotencyKeyV1::new("begin").unwrap(),
            deadline: deadline(),
            input: SessionBeginV1 {
                initial_goal: None,
                preconditions: SessionIdentityPreconditionsWireV1 {
                    expected_session_id: session.clone(),
                    expected_session_revision: Revision::ZERO,
                },
            },
        },
        &clock,
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    let attempt = AttemptId::new(
        begin.as_value()["result"]["attempt"]["attempt_id"]
            .as_str()
            .unwrap(),
    )
    .unwrap();
    execute_item_v1(
        &resolver,
        DirectItemRequestV1 {
            selector: selector(root.path()),
            expected_workspace_id: None,
            request_id: RequestIdV1::new(RESPONSE).unwrap(),
            key: IdempotencyKeyV1::new("attach").unwrap(),
            deadline: deadline(),
            command: DirectItemCommandV1::Attach(ItemAttachV1 {
                item_id: ItemId::new("report").unwrap(),
                source: ItemAttachSourceV1::Path {
                    path: "report.txt".into(),
                    media_type: None,
                },
                preconditions: ItemMutationPreconditionsWireV1 {
                    expected_session_id: session,
                    expected_attempt_id: attempt,
                    expected_item_revision: Revision::ZERO,
                },
            }),
        },
        &clock,
    )
    .unwrap();
    let healthy = doctor_workspace_v1(&resolver, read(root.path()), true, &clock).unwrap();
    assert_eq!(
        healthy.as_value()["result"]["current_required_local_artifacts_checked"],
        1
    );
    std::fs::write(root.path().join("report.txt"), "Changed report").unwrap();
    let before = files(&root.path().join(".podway"));
    assert!(matches!(
        doctor_workspace_v1(&resolver, read(root.path()), true, &NoEffects),
        Err(DirectWorkspaceErrorV1::Boundary(_))
    ));
    assert_eq!(files(&root.path().join(".podway")), before);
    doctor_workspace_v1(&resolver, read(root.path()), false, &clock).unwrap();
}
