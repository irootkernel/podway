//! Real Git worktrees and current-generation workspace commands without a daemon.
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
    // Temporary roots can have symlinked parents, including macOS /var.
    let path = std::fs::canonicalize(path).expect("canonical fixture worktree");
    WorktreeSelectorV1::new(
        WORKTREE_SELECTOR_VERSION_V1,
        None,
        LosslessPathV1::from_raw_bytes(
            path.as_os_str().as_bytes(),
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

#[test]
fn workspace_fixture_resolves_a_symlinked_temporary_parent() {
    let parent = tempfile::tempdir().unwrap();
    let root = fixture();
    let alias = parent.path().join("temporary-parent");
    std::os::unix::fs::symlink(root.path().parent().unwrap(), &alias).unwrap();
    let selected = alias.join(root.path().file_name().unwrap());
    let resolver = DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest());
    let output = initialize_workspace_v1(
        &resolver,
        mutation(&selected, "init"),
        false,
        &Clock(AtomicUsize::new(0)),
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    let shown = show_workspace_v1(&resolver, read(&selected), &Clock(AtomicUsize::new(0))).unwrap();
    assert_eq!(
        shown.as_value()["workspace"],
        output.as_value()["workspace"]
    );
    assert!(root.path().join(".podway/runtime/state.sqlite3").is_file());
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
fn existing_plain_init_refuses_new_keys_but_preserves_original_replay_and_repair() {
    use podway_engine::workspace::{DirectWorkspaceResolutionErrorV1, WorkspaceResolutionErrorV1};
    let root = fixture();
    let resolver = DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest());
    let original = initialize_workspace_v1(
        &resolver,
        mutation(root.path(), "init"),
        false,
        &Clock(AtomicUsize::new(0)),
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    let before = files(&root.path().join(".podway"));
    let rejected = initialize_workspace_v1(
        &resolver,
        mutation(root.path(), "second-init"),
        false,
        &NoEffects,
        &NoEffects,
    );
    assert!(matches!(
        rejected,
        Err(DirectWorkspaceErrorV1::Resolution(
            DirectWorkspaceResolutionErrorV1::Resolution(
                WorkspaceResolutionErrorV1::BootstrapBindingAlreadyPresent
            )
        ))
    ));
    assert_eq!(files(&root.path().join(".podway")), before);
    let (_, store) = resolver
        .resolve_existing(selector(root.path()), None, deadline())
        .unwrap();
    store
        .read(deadline(), |snapshot| {
            assert!(
                snapshot
                    .outcome(&IdempotencyKeyV1::new("second-init").unwrap())?
                    .is_none()
            );
            assert_eq!(snapshot.committed_sequence()?, 1);
            Ok(())
        })
        .unwrap();
    assert_eq!(
        initialize_workspace_v1(
            &resolver,
            mutation(root.path(), "init"),
            false,
            &NoEffects,
            &NoEffects,
        )
        .unwrap(),
        original
    );
    let repair = initialize_workspace_v1(
        &resolver,
        mutation(root.path(), "repair-init"),
        true,
        &Clock(AtomicUsize::new(0)),
        &NoEffects,
    )
    .unwrap();
    assert_eq!(
        repair.as_value()["workspace"]["uuid"],
        original.as_value()["workspace"]["uuid"]
    );
    assert_eq!(
        repair.as_value()["workspace"]["latest_workspace_sequence"],
        2
    );
    assert_eq!(
        initialize_workspace_v1(
            &resolver,
            mutation(root.path(), "repair-init"),
            true,
            &NoEffects,
            &NoEffects,
        )
        .unwrap(),
        repair
    );
}

#[test]
fn plain_init_refuses_an_outcome_pruned_before_its_authoritative_lookup() {
    use podway_engine::workspace::{DirectWorkspaceResolutionErrorV1, WorkspaceResolutionErrorV1};
    use podway_store::direct_store::DirectTransactionBoundaryV1;
    use std::sync::{Arc, Mutex};
    const CHILD_ROOT: &str = "PODWAY_V2DFC006_RETENTION_RACE_CHILD_ROOT";
    const TEST: &str = "int_v2dfc006_workspace::plain_init_refuses_an_outcome_pruned_before_its_authoritative_lookup";
    if let Some(root) = std::env::var_os(CHILD_ROOT) {
        let resolver = DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest());
        repair_workspace_v1(
            &resolver,
            mutation(std::path::Path::new(&root), "prune-original-outcome"),
            &Clock(AtomicUsize::new(0)),
        )
        .unwrap();
        return;
    }
    // Both an exact original init and a conflicting repair key can disappear
    // after the preliminary lookup. Neither may become a new plain init.
    for conflicting in [false, true] {
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
        let key = if conflicting {
            repair_workspace_v1(
                &resolver,
                mutation(root.path(), "original-repair"),
                &Clock(AtomicUsize::new(0)),
            )
            .unwrap();
            "original-repair"
        } else {
            "init"
        };
        for index in 0..99 {
            repair_workspace_v1(
                &resolver,
                mutation(root.path(), &format!("retained-repair-{index}")),
                &Clock(AtomicUsize::new(0)),
            )
            .unwrap();
        }
        let (_, store) = resolver
            .resolve_existing(selector(root.path()), None, deadline())
            .unwrap();
        store
            .read(deadline(), |snapshot| {
                assert!(
                    snapshot
                        .outcome(&IdempotencyKeyV1::new(key).unwrap())?
                        .is_some()
                );
                Ok(())
            })
            .unwrap();
        let root_path = root.path().to_path_buf();
        let after_pruning = Arc::new(Mutex::new(None));
        let captured = after_pruning.clone();
        let racing = DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest())
            .with_store_boundary_observer_v1(move |boundary| {
                if boundary == DirectTransactionBoundaryV1::BeforeBegin {
                    let child = Command::new(std::env::current_exe().unwrap())
                        .args(["--exact", TEST, "--nocapture"])
                        .env(CHILD_ROOT, &root_path)
                        .output()
                        .unwrap();
                    assert!(child.status.success(), "{child:?}");
                    let mut preserved = files(&root_path.join(".podway"));
                    // BEGIN/rollback may change only SQLite's volatile index.
                    preserved.remove("runtime/state.sqlite3-shm");
                    *captured.lock().unwrap() = Some(preserved);
                }
            });
        let rejected = initialize_workspace_v1(
            &racing,
            mutation(root.path(), key),
            false,
            &NoEffects,
            &NoEffects,
        );
        assert!(matches!(
            rejected,
            Err(DirectWorkspaceErrorV1::Resolution(
                DirectWorkspaceResolutionErrorV1::Resolution(
                    WorkspaceResolutionErrorV1::BootstrapBindingAlreadyPresent
                )
            ))
        ));
        let mut after = files(&root.path().join(".podway"));
        after.remove("runtime/state.sqlite3-shm");
        assert_eq!(Some(after), *after_pruning.lock().unwrap());
        store
            .read(deadline(), |snapshot| {
                assert!(
                    snapshot
                        .outcome(&IdempotencyKeyV1::new(key).unwrap())?
                        .is_none()
                );
                assert_eq!(snapshot.committed_sequence()?, 101 + u64::from(conflicting));
                Ok(())
            })
            .unwrap();
    }
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

#[test]
fn direct_config_admission_rejects_nonprod_and_invalid_bytes_before_store_or_effects() {
    use podway_engine::direct_api::{
        DirectCommandV1, DirectRequestV1, DirectResponseV1, execute_direct_v1,
    };
    use podway_protocol::Rfc3339MillisV1;
    let root = fixture();
    let base = DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest());
    initialize_workspace_v1(
        &base,
        mutation(root.path(), "config-init"),
        false,
        &Clock(AtomicUsize::new(0)),
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    let opens = std::sync::Arc::new(AtomicUsize::new(0));
    let observed = opens.clone();
    let resolver = DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest())
        .with_store_boundary_observer_v1(move |_| {
            observed.fetch_add(1, Ordering::SeqCst);
        });
    let config = root.path().join(".podway/config.yaml");
    for bytes in [
        b"schema: podway.workspace/v2\nmode: dev\n".to_vec(),
        b"schema: podway.workspace/v2\nmode: release-qualification\n".to_vec(),
        b"schema: invalid\n".to_vec(),
        vec![0xff],
        vec![b'x'; podway_config::MAX_WORKSPACE_CONFIG_BYTES_V1 + 1],
    ] {
        std::fs::write(&config, bytes).unwrap();
        let before = files(&root.path().join(".podway"));
        for (index, command) in [
            DirectCommandV1::WorkspaceShow,
            DirectCommandV1::WorkspaceRepair,
            DirectCommandV1::WorkspaceInit { repair: false },
        ]
        .into_iter()
        .enumerate()
        {
            let response = execute_direct_v1(
                &resolver,
                DirectRequestV1 {
                    request_id: RequestIdV1::new(RESPONSE).unwrap(),
                    selector: selector(root.path()),
                    expected_workspace_id: None,
                    command,
                    idempotency_key: Some(
                        IdempotencyKeyV1::new(format!("invalid-config-{index}")).unwrap(),
                    ),
                    deadline: deadline(),
                    observed_at: Rfc3339MillisV1::new("2026-10-07T00:00:00.000Z").unwrap(),
                    timeout_ms: 5000,
                },
                &NoEffects,
                &NoEffects,
            )
            .unwrap();
            let DirectResponseV1::Failure(error) = response else {
                panic!("invalid config admitted")
            };
            let value = error.as_value();
            assert_eq!(value["code"], "WORKSPACE_CONFIG_INVALID");
            assert_eq!(value["exit_code"], 5);
            assert!(
                value["details"]["guidance"]
                    .as_str()
                    .unwrap()
                    .contains("Remove")
            );
            assert_eq!(files(&root.path().join(".podway")), before);
            assert_eq!(
                opens.load(Ordering::SeqCst),
                0,
                "invalid config opened Store before rejection"
            );
        }
    }
    let outside = tempfile::tempdir().unwrap();
    let outside_config = outside.path().join("config.yaml");
    std::fs::write(&outside_config, b"schema: podway.workspace/v1\n").unwrap();
    std::fs::remove_file(&config).unwrap();
    std::os::unix::fs::symlink(&outside_config, &config).unwrap();
    assert!(matches!(
        resolver.resolve_existing(selector(root.path()), None, deadline()),
        Err(
            podway_engine::workspace::DirectWorkspaceResolutionErrorV1::Resolution(
                podway_engine::workspace::WorkspaceResolutionErrorV1::WorkspaceConfigInvalid
            )
        )
    ));
    assert_eq!(opens.load(Ordering::SeqCst), 0);
    assert_eq!(
        std::fs::read(&outside_config).unwrap(),
        b"schema: podway.workspace/v1\n"
    );
    std::fs::remove_file(&config).unwrap();
    for bytes in [
        b"schema: podway.workspace/v1\njob_queue: {max_pending: 1}\n".as_slice(),
        b"schema: podway.workspace/v2\n",
        b"schema: podway.workspace/v2\nmode: prod\n",
    ] {
        std::fs::write(&config, bytes).unwrap();
        show_workspace_v1(
            &base,
            DirectWorkspaceReadRequestV1 {
                selector: selector(root.path()),
                expected_workspace_id: None,
                request_id: RequestIdV1::new(RESPONSE).unwrap(),
                deadline: deadline(),
            },
            &Clock(AtomicUsize::new(0)),
        )
        .unwrap();
    }
    std::fs::write(&config, b"schema: podway.workspace/v2\nmode: dev\n").unwrap();
    let request = podway_engine::direct_maintenance::DirectMaintenanceRequestV1 {
        selector: selector(root.path()),
        expected_workspace_id: None,
        request_id: RequestIdV1::new(RESPONSE).unwrap(),
        key: IdempotencyKeyV1::new("invalid-config-reset").unwrap(),
        confirmed: true,
        force: false,
        deadline: deadline(),
    };
    podway_engine::direct_maintenance::reset_all_v1(
        &base,
        request,
        &Clock(AtomicUsize::new(0)),
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    let request = podway_engine::direct_maintenance::DirectMaintenanceRequestV1 {
        selector: selector(root.path()),
        expected_workspace_id: None,
        request_id: RequestIdV1::new(RESPONSE).unwrap(),
        key: IdempotencyKeyV1::new("invalid-config-remove").unwrap(),
        confirmed: true,
        force: true,
        deadline: deadline(),
    };
    podway_engine::direct_maintenance::remove_workspace_v1(
        &base,
        request,
        &Clock(AtomicUsize::new(0)),
    )
    .unwrap();
    assert!(!root.path().join(".podway").exists());
}

#[test]
fn bootstrap_rejects_existing_nonprod_config_without_creating_runtime_or_store() {
    let root = fixture();
    std::fs::create_dir(root.path().join(".podway")).unwrap();
    let config = root.path().join(".podway/config.yaml");
    std::fs::write(&config, b"schema: podway.workspace/v2\nmode: dev\n").unwrap();
    let before = files(&root.path().join(".podway"));
    let resolver = DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest());
    let error = initialize_workspace_v1(
        &resolver,
        mutation(root.path(), "invalid-bootstrap"),
        false,
        &NoEffects,
        &NoEffects,
    )
    .unwrap_err();
    assert!(matches!(
        error,
        DirectWorkspaceErrorV1::Resolution(
            podway_engine::workspace::DirectWorkspaceResolutionErrorV1::Resolution(
                podway_engine::workspace::WorkspaceResolutionErrorV1::WorkspaceConfigInvalid
            )
        )
    ));
    assert_eq!(files(&root.path().join(".podway")), before);
    assert!(!root.path().join(".podway/runtime").exists());
}

/// STATE-002: the doctor reuses the verifier's admitted current graph in both
/// modes. Each run performs exactly one current-graph admission and one
/// disposition load; the deep run still rehashes its required artifacts from
/// the admitted graph before the clock, and the shallow run skips them.
#[test]
fn state002_doctor_reuses_verifier_admission_deep_and_non_deep() {
    use super::int_v2dfc006_domain_ports::state002_executes;
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
    use podway_store::test_support::query_account::QueryAccountant;
    // Fixture: one current running session with one required local artifact
    // and no archived sessions, from the deep-doctor family.
    let root = fixture();
    let resolver = DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest());
    let clock = Clock(AtomicUsize::new(0));
    initialize_workspace_v1(
        &resolver,
        mutation(root.path(), "state002-init"),
        false,
        &clock,
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    let source = br#"{"schema":"podway.procedure/v2","id":"diagnostic-state002","version":"1","name":"Diagnostic","purpose":"Check shared admitted state.",
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
            key: Some(IdempotencyKeyV1::new("state002-start").unwrap()),
            deadline: deadline(),
            command: DirectStartCommandV1::Start(SessionStartV1 {
                source: SessionStartSourceV1::Procedure {
                    procedure: "procedure.json".into(),
                },
                expected_procedure_digest: Some(prepared.digest().clone()),
                task_title: "Diagnose shared admitted state".into(),
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
            key: IdempotencyKeyV1::new("state002-begin").unwrap(),
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
            key: IdempotencyKeyV1::new("state002-attach").unwrap(),
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
    let check_status = |value: &serde_json::Value, id: &str| -> serde_json::Value {
        value["result"]["checks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|check| check["id"] == id)
            .unwrap_or_else(|| panic!("missing doctor check {id}"))["status"]
            .clone()
    };
    for deep in [false, true] {
        let accountant = QueryAccountant::start();
        let healthy = doctor_workspace_v1(&resolver, read(root.path()), deep, &clock).unwrap();
        let report = accountant.finish();
        let value = healthy.as_value();
        assert_eq!(value["result"]["healthy"], true);
        assert_eq!(value["result"]["deep"], deep);
        assert_eq!(
            state002_executes(
                &report.sql,
                "SELECT COUNT(*) FROM v2_task_sessions WHERE activity = 'current'"
            ),
            1,
            "doctor ({deep}) must admit the current graph exactly once"
        );
        assert_eq!(
            state002_executes(&report.sql, "FROM v2_terminal_dispositions"),
            1,
            "doctor ({deep}) must load dispositions exactly once"
        );
        if deep {
            assert_eq!(
                value["result"]["current_required_local_artifacts_checked"],
                1
            );
            assert_eq!(
                check_status(value, "current_required_local_artifacts"),
                "pass"
            );
        } else {
            assert_eq!(
                value["result"]["current_required_local_artifacts_checked"],
                0
            );
            assert_eq!(
                check_status(value, "current_required_local_artifacts"),
                "skipped"
            );
        }
    }
}
