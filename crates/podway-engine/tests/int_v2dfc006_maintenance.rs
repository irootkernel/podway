//! Engine-owned maintenance markers, exclusion and exact retry in disposable worktrees.
use podway_core::{
    AttemptId, BlockerId, DelegationReceiptIdV1, ProcedureSnapshotId, SessionId, Sha256Digest,
    UnixMillis, WorkspaceId,
};
use podway_engine::{
    direct_execution::{DirectIdSourceV1, NativeDirectIdSourceV1},
    direct_maintenance::{
        DirectMaintenanceBoundaryV1 as Boundary, DirectMaintenanceErrorV1 as Error,
        DirectMaintenanceRequestV1 as Request, remove_workspace_v1,
        remove_workspace_with_boundary_observer_v1, reset_all_v1,
        reset_all_with_boundary_observer_v1,
    },
    direct_workspace::{DirectWorkspaceMutationRequestV1, initialize_workspace_v1},
    execution::ExecutionClockV1,
    workspace::DirectWorkspaceResolverV1,
};
use podway_git::{
    DiagnosticPathDisplayV1, LosslessPathV1, NativeGitResolverV1, WORKTREE_SELECTOR_VERSION_V1,
    WorktreeSelectorV1,
};
use podway_protocol::{RequestIdV1, validate_daemon_free_document_v1};
use podway_store::{
    IdempotencyKeyV1,
    direct_store::{DirectTransactionBoundaryV1, SharedMaintenanceLeaseV1},
};
use sha2::{Digest as _, Sha256};
use std::{
    collections::BTreeMap,
    os::unix::ffi::OsStrExt,
    process::Command,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tempfile::TempDir;
const RESPONSE: &str = "00000000-0000-4000-8000-000000000001";
const OTHER: &str = "00000000-0000-4000-8000-000000000002";

#[test]
fn qualified_retirement_observer_reads_the_actual_generation_under_its_native_lease() {
    use podway_engine::direct_maintenance::{
        remove_workspace_with_lease_observers_v1, reset_all_with_lease_observers_v1,
    };
    let root = fixture();
    let original = initialize(root.path());
    let resolver = DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest());
    podway_engine::direct_workspace::repair_workspace_v1(
        &resolver,
        DirectWorkspaceMutationRequestV1 {
            selector: selector(root.path()),
            expected_workspace_id: Some(original.clone()),
            request_id: RequestIdV1::new(OTHER).unwrap(),
            key: IdempotencyKeyV1::new("qualified-repair").unwrap(),
            deadline: deadline(),
        },
        &Clock,
    )
    .unwrap();
    let retired = Arc::new(Mutex::new(Vec::new()));
    let acquisitions = Arc::new(Mutex::new(Vec::new()));
    let callback = |retired: Arc<Mutex<Vec<WorkspaceId>>>,
                    acquisitions: Arc<Mutex<Vec<Duration>>>| {
        move |boundary: Boundary,
              store: &podway_store::direct_store::DirectStoreV1,
              lease: &podway_store::direct_store::ExclusiveMaintenanceLeaseV1| {
            match boundary {
                Boundary::LeaseAcquired { wait } => acquisitions.lock().unwrap().push(wait),
                Boundary::BeforeRuntimeDestruction => {
                    let uuid=store.read_under_exclusive_lease(lease,deadline(),|snapshot| {
                        assert!(snapshot.latest_outcome()?.is_some());
                        let keys=snapshot.retained_outcome_keys_for_qualification_v1(2)?;
                        assert!(!keys.is_empty());
                        for key in &keys {assert!(snapshot.outcome(key)?.is_some());}
                        assert!(matches!(snapshot.retained_outcome_keys_for_qualification_v1(0),
                            Err(podway_store::direct_store::DirectStoreErrorV1::InvalidInput {..})));
                        if keys.len()==2 {
                            assert_eq!(keys.iter().map(IdempotencyKeyV1::as_str).collect::<Vec<_>>(),vec!["init","qualified-repair"]);
                            assert!(matches!(snapshot.retained_outcome_keys_for_qualification_v1(1),
                                Err(podway_store::direct_store::DirectStoreErrorV1::InvalidInput {..})));
                        }
                        Ok(snapshot.binding().identity().workspace_uuid().clone())
                    }).unwrap();
                    retired.lock().unwrap().push(uuid);
                }
                _ => panic!("unexpected qualified boundary"),
            }
        }
    };
    let reset = reset_all_with_lease_observers_v1(
        &resolver,
        request(root.path(), "qualified-reset", Some(original.clone())),
        &Clock,
        &NativeDirectIdSourceV1,
        |_| {},
        callback(retired.clone(), acquisitions.clone()),
    )
    .unwrap();
    let next = WorkspaceId::new(reset.as_value()["workspace"]["uuid"].as_str().unwrap()).unwrap();
    remove_workspace_with_lease_observers_v1(
        &resolver,
        request(root.path(), "qualified-remove", Some(next.clone())),
        &Clock,
        |_| {},
        callback(retired.clone(), acquisitions.clone()),
    )
    .unwrap();
    assert_eq!(*retired.lock().unwrap(), vec![original, next]);
    assert_eq!(acquisitions.lock().unwrap().len(), 2);
    assert!(!root.path().join(".podway").exists());
}

struct Clock;
impl ExecutionClockV1 for Clock {
    fn now(&self) -> UnixMillis {
        UnixMillis::new(1_700_000_000_100)
    }
}
struct NoEffects;
impl ExecutionClockV1 for NoEffects {
    fn now(&self) -> UnixMillis {
        panic!("replay/rejection read clock")
    }
}
impl DirectIdSourceV1 for NoEffects {
    fn next_workspace_id(&self) -> WorkspaceId {
        panic!("replay/rejection generated workspace")
    }
    fn next_session_id(&self) -> SessionId {
        panic!("unexpected session")
    }
    fn next_attempt_id(&self) -> AttemptId {
        panic!("unexpected attempt")
    }
    fn next_blocker_id(&self) -> BlockerId {
        panic!("unexpected blocker")
    }
    fn next_procedure_snapshot_id(&self) -> ProcedureSnapshotId {
        panic!("unexpected snapshot")
    }
    fn next_delegation_receipt_id(&self) -> DelegationReceiptIdV1 {
        panic!("unexpected receipt")
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
fn initialize(root: &std::path::Path) -> WorkspaceId {
    let resolver = DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest());
    let output = initialize_workspace_v1(
        &resolver,
        DirectWorkspaceMutationRequestV1 {
            selector: selector(root),
            expected_workspace_id: None,
            request_id: RequestIdV1::new(RESPONSE).unwrap(),
            key: IdempotencyKeyV1::new("init").unwrap(),
            deadline: deadline(),
        },
        false,
        &Clock,
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    WorkspaceId::new(output.as_value()["workspace"]["uuid"].as_str().unwrap()).unwrap()
}
fn request(root: &std::path::Path, key: &str, expected: Option<WorkspaceId>) -> Request {
    Request {
        selector: selector(root),
        expected_workspace_id: expected,
        request_id: RequestIdV1::new(OTHER).unwrap(),
        key: IdempotencyKeyV1::new(key).unwrap(),
        confirmed: true,
        force: false,
        deadline: deadline(),
    }
}
fn files(root: &std::path::Path) -> BTreeMap<String, String> {
    fn visit(root: &std::path::Path, path: &std::path::Path, out: &mut BTreeMap<String, String>) {
        for entry in std::fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                visit(root, &path, out);
            } else {
                out.insert(
                    path.strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                    format!("{:x}", Sha256::digest(std::fs::read(path).unwrap())),
                );
            }
        }
    }
    let mut out = BTreeMap::new();
    visit(root, root, &mut out);
    out
}
#[test]
fn reset_all_replaces_generation_and_replays_before_old_workspace_fences() {
    let root = fixture();
    let old = initialize(root.path());
    let resolver = DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest());
    let input = request(root.path(), "reset", Some(old.clone()));
    let output = reset_all_v1(&resolver, input.clone(), &Clock, &NativeDirectIdSourceV1).unwrap();
    assert!(validate_daemon_free_document_v1(
        "podway.output/v4",
        output.as_value()
    ));
    let fresh = WorkspaceId::new(output.as_value()["workspace"]["uuid"].as_str().unwrap()).unwrap();
    assert_ne!(fresh, old);
    assert_eq!(
        output.as_value()["workspace"]["latest_workspace_sequence"],
        1
    );
    assert_eq!(
        reset_all_v1(&resolver, input.clone(), &NoEffects, &NoEffects).unwrap(),
        output
    );
    let mut changed = input;
    changed.expected_workspace_id = None;
    assert_eq!(
        reset_all_v1(&resolver, changed, &NoEffects, &NoEffects).unwrap(),
        output
    );
    let (_, store) = resolver
        .resolve_existing(selector(root.path()), Some(&fresh), deadline())
        .unwrap();
    store
        .read(deadline(), |snapshot| {
            assert!(snapshot.current_graph()?.is_none());
            assert!(
                snapshot
                    .outcome(&IdempotencyKeyV1::new("init").unwrap())?
                    .is_none()
            );
            assert!(
                snapshot
                    .outcome(&IdempotencyKeyV1::new("reset").unwrap())?
                    .is_some()
            );
            Ok(())
        })
        .unwrap();
    assert!(!store.layout().intent_marker().exists());
    assert!(!store.layout().reset_marker().exists());
}
#[test]
fn destructive_guards_and_shared_exclusion_reject_without_clock_identifiers_or_byte_changes() {
    let root = fixture();
    let old = initialize(root.path());
    let resolver = DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest());
    let before = files(&root.path().join(".podway"));
    let mut input = request(root.path(), "reset", Some(old.clone()));
    input.confirmed = false;
    assert!(matches!(
        reset_all_v1(&resolver, input, &NoEffects, &NoEffects),
        Err(Error::ConfirmationRequired)
    ));
    let input = request(root.path(), "wrong", Some(WorkspaceId::new(OTHER).unwrap()));
    assert!(matches!(
        remove_workspace_v1(&resolver, input, &NoEffects),
        Err(Error::Workspace(_))
    ));
    let (_, store) = resolver
        .resolve_existing(selector(root.path()), Some(&old), deadline())
        .unwrap();
    let shared = SharedMaintenanceLeaseV1::acquire(store.layout(), deadline()).unwrap();
    let mut input = request(root.path(), "blocked", Some(old));
    input.deadline = Instant::now() + Duration::from_millis(30);
    assert!(matches!(
        reset_all_v1(&resolver, input, &NoEffects, &NoEffects),
        Err(Error::Workspace(_))
    ));
    drop(shared);
    assert_eq!(files(&root.path().join(".podway")), before);
}
#[test]
fn legacy_markers_are_deleted_only_by_confirmed_reset_all_without_conversion() {
    for marker in [
        "reset.marker",
        "workspace-removal.marker",
        "development-v2.marker",
        "../mode-switch.json",
    ] {
        let root = fixture();
        initialize(root.path());
        std::fs::write(
            root.path().join(".podway/runtime").join(marker),
            "daemon-era marker",
        )
        .unwrap();
        let resolver = DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest());
        let before = files(&root.path().join(".podway"));
        assert!(matches!(
            remove_workspace_v1(&resolver, request(root.path(), "remove", None), &NoEffects),
            Err(Error::Workspace(_))
        ));
        assert_eq!(files(&root.path().join(".podway")), before);
        let output = reset_all_v1(
            &resolver,
            request(root.path(), "reset", None),
            &Clock,
            &NativeDirectIdSourceV1,
        )
        .unwrap();
        assert!(validate_daemon_free_document_v1(
            "podway.output/v4",
            output.as_value()
        ));
        assert!(!root.path().join(".podway/runtime").join(marker).exists());
        let (_, store) = resolver
            .resolve_existing(selector(root.path()), None, deadline())
            .unwrap();
        store.verify_integrity(deadline()).unwrap();
    }
}
#[test]
fn maintenance_crash_child() {
    let Some(path) = std::env::var_os("PODWAY_MAINTENANCE_FIXTURE") else {
        return;
    };
    let action = std::env::var("PODWAY_MAINTENANCE_ACTION").unwrap();
    let phase = std::env::var("PODWAY_MAINTENANCE_PHASE").unwrap();
    let uuid = WorkspaceId::new(std::env::var("PODWAY_MAINTENANCE_UUID").unwrap()).unwrap();
    let resolver = DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest());
    let observer = move |actual| {
        let expected = match phase.as_str() {
            "intent" => Boundary::IntentPublished,
            "marker" => Boundary::MarkerPublished,
            "cleared" => Boundary::RuntimeCleared,
            "phase" => Boundary::PhaseAdvanced,
            "final" => Boundary::BeforeFinalRemoval,
            "first-marker" => Boundary::AfterFirstRemovalMarker,
            "before_begin" => Boundary::StoreBoundary(DirectTransactionBoundaryV1::BeforeBegin),
            "before_outcome" => {
                Boundary::StoreBoundary(DirectTransactionBoundaryV1::AfterStateBeforeOutcome)
            }
            "before_publication" => {
                Boundary::StoreBoundary(DirectTransactionBoundaryV1::BeforeMaintenancePublication)
            }
            "after_publication" => {
                Boundary::StoreBoundary(DirectTransactionBoundaryV1::AfterCommitBeforeOutput)
            }
            _ => panic!("unknown owned maintenance crash phase"),
        };
        if actual == expected {
            if phase == "first-marker" {
                let ready = std::env::var_os("PODWAY_MAINTENANCE_READY").unwrap();
                std::fs::write(ready, b"paused").unwrap();
                loop {
                    std::thread::sleep(Duration::from_secs(1));
                }
            }
            std::process::exit(77);
        }
    };
    if action == "reset" {
        reset_all_with_boundary_observer_v1(
            &resolver,
            request(std::path::Path::new(&path), "reset", Some(uuid)),
            &Clock,
            &NativeDirectIdSourceV1,
            observer,
        )
        .unwrap();
    } else {
        remove_workspace_with_boundary_observer_v1(
            &resolver,
            request(std::path::Path::new(&path), "remove", Some(uuid)),
            &Clock,
            observer,
        )
        .unwrap();
    }
    panic!("actual maintenance boundary was not reached");
}
fn crash(root: &std::path::Path, uuid: &WorkspaceId, action: &str, phase: &str) {
    if phase == "first-marker" {
        use std::os::unix::process::ExitStatusExt;
        let ready = root.join("removal-paused");
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "int_v2dfc006_maintenance::maintenance_crash_child",
                "--nocapture",
            ])
            .env("PODWAY_MAINTENANCE_FIXTURE", root)
            .env("PODWAY_MAINTENANCE_UUID", uuid.as_str())
            .env("PODWAY_MAINTENANCE_ACTION", action)
            .env("PODWAY_MAINTENANCE_PHASE", phase)
            .env("PODWAY_MAINTENANCE_READY", &ready)
            .spawn()
            .unwrap();
        let limit = deadline();
        while !ready.exists() {
            if let Some(status) = child.try_wait().unwrap() {
                panic!("removal exited before the marker barrier: {status}");
            }
            if Instant::now() >= limit {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("removal did not reach the bounded marker barrier");
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        child.kill().unwrap();
        assert_eq!(child.wait().unwrap().signal(), Some(9));
        return;
    }
    let status = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "int_v2dfc006_maintenance::maintenance_crash_child",
            "--nocapture",
        ])
        .env("PODWAY_MAINTENANCE_FIXTURE", root)
        .env("PODWAY_MAINTENANCE_UUID", uuid.as_str())
        .env("PODWAY_MAINTENANCE_ACTION", action)
        .env("PODWAY_MAINTENANCE_PHASE", phase)
        .status()
        .unwrap();
    assert_eq!(
        status.code(),
        Some(77),
        "{action}/{phase}: owned Engine boundary was not reached"
    );
}
#[test]
fn exact_reset_retry_resumes_all_owned_marker_and_private_publication_windows() {
    for phase in [
        "intent",
        "marker",
        "cleared",
        "phase",
        "before_begin",
        "before_outcome",
        "before_publication",
        "after_publication",
    ] {
        let root = fixture();
        let old = initialize(root.path());
        crash(root.path(), &old, "reset", phase);
        let resolver = DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest());
        assert!(
            resolver
                .resolve_existing(selector(root.path()), None, deadline())
                .is_err()
        );
        assert!(matches!(
            reset_all_v1(
                &resolver,
                request(root.path(), "other", Some(old.clone())),
                &NoEffects,
                &NoEffects
            ),
            Err(Error::Workspace(_))
        ));
        let input = request(root.path(), "reset", Some(old));
        let result = reset_all_v1(&resolver, input.clone(), &NoEffects, &NoEffects).unwrap();
        assert!(validate_daemon_free_document_v1(
            "podway.output/v4",
            result.as_value()
        ));
        assert_eq!(
            reset_all_v1(&resolver, input, &NoEffects, &NoEffects).unwrap(),
            result
        );
        let (_, store) = resolver
            .resolve_existing(selector(root.path()), None, deadline())
            .unwrap();
        store.verify_integrity(deadline()).unwrap();
    }
}
#[test]
fn exact_removal_retry_resumes_markers_then_absence_destroys_retention() {
    for phase in ["intent", "marker", "cleared", "phase", "final"] {
        let root = fixture();
        let old = initialize(root.path());
        crash(root.path(), &old, "remove", phase);
        let resolver = DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest());
        assert!(
            resolver
                .resolve_existing(selector(root.path()), None, deadline())
                .is_err()
        );
        assert!(matches!(
            remove_workspace_v1(
                &resolver,
                request(root.path(), "other", Some(old.clone())),
                &NoEffects
            ),
            Err(Error::Workspace(_))
        ));
        let input = request(root.path(), "remove", Some(old.clone()));
        let result = remove_workspace_v1(&resolver, input.clone(), &NoEffects).unwrap();
        assert!(validate_daemon_free_document_v1(
            "podway.output/v4",
            result.as_value()
        ));
        assert_eq!(result.as_value()["result"]["workspace_uuid"], old.as_str());
        assert_eq!(result.as_value()["result"]["already_absent"], false);
        assert!(!root.path().join(".podway").exists());
        let absent = remove_workspace_v1(&resolver, input, &Clock).unwrap();
        assert_eq!(
            absent.as_value()["result"]["workspace_uuid"],
            serde_json::Value::Null
        );
        assert_eq!(absent.as_value()["result"]["already_absent"], true);
    }
}
#[test]
fn interrupted_removal_refuses_changed_explicit_and_selector_workspace_fences() {
    use podway_git::DurableWorktreeIdentityV1;
    use podway_store::direct_store::DirectStoreErrorV1;
    for phase in [
        "intent",
        "marker",
        "cleared",
        "phase",
        "final",
        "first-marker",
    ] {
        for selector_fence in [false, true] {
            let root = fixture();
            let original = initialize(root.path());
            let resolver = DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest());
            let (resolved, _) = resolver.resolve_candidate(selector(root.path())).unwrap();
            let identity = resolved.identity().clone();
            crash(root.path(), &original, "remove", phase);
            let before = files(&root.path().join(".podway"));
            if phase == "first-marker" {
                assert_eq!(
                    before.keys().map(String::as_str).collect::<Vec<_>>(),
                    vec!["runtime/maintenance-intent.json"]
                );
            }
            let wrong = WorkspaceId::new(OTHER).unwrap();
            let mut retry = request(root.path(), "remove", Some(original.clone()));
            if selector_fence {
                retry.selector = WorktreeSelectorV1::new(
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
            } else {
                retry.expected_workspace_id = Some(wrong.clone());
            }
            let rejected = remove_workspace_v1(&resolver, retry, &NoEffects);
            assert!(
                matches!(rejected, Err(Error::Workspace(
                podway_engine::direct_workspace::DirectWorkspaceErrorV1::Store(
                    DirectStoreErrorV1::ExpectedWorkspaceUuidMismatch {expected, actual}
                )
            )) if expected == wrong && actual == original),
                "{phase}/{selector_fence}"
            );
            assert_eq!(files(&root.path().join(".podway")), before);
            let resumed = remove_workspace_v1(
                &resolver,
                request(root.path(), "remove", Some(original.clone())),
                &NoEffects,
            )
            .unwrap();
            assert_eq!(
                resumed.as_value()["result"]["workspace_uuid"],
                original.as_str()
            );
            assert!(!root.path().join(".podway").exists());
            assert!(root.path().join(".git").is_dir());
        }
    }
}

#[test]
fn sigkill_between_final_removal_markers_preserves_exact_retry_and_original_response() {
    use std::os::unix::process::ExitStatusExt;
    let root = fixture();
    let old = initialize(root.path());
    let ready = root.path().join("removal-paused");
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "int_v2dfc006_maintenance::maintenance_crash_child",
            "--nocapture",
        ])
        .env("PODWAY_MAINTENANCE_FIXTURE", root.path())
        .env("PODWAY_MAINTENANCE_UUID", old.as_str())
        .env("PODWAY_MAINTENANCE_ACTION", "remove")
        .env("PODWAY_MAINTENANCE_PHASE", "first-marker")
        .env("PODWAY_MAINTENANCE_READY", &ready)
        .spawn()
        .unwrap();
    let limit = deadline();
    while !ready.exists() {
        if let Some(status) = child.try_wait().unwrap() {
            panic!("removal exited before the marker barrier: {status}");
        }
        if Instant::now() >= limit {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("removal did not reach the bounded marker barrier");
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    child.kill().unwrap();
    assert_eq!(child.wait().unwrap().signal(), Some(9));
    let resolver = DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest());
    assert!(
        resolver
            .resolve_existing(selector(root.path()), None, deadline())
            .is_err()
    );
    assert!(matches!(
        remove_workspace_v1(
            &resolver,
            request(root.path(), "other", Some(old.clone())),
            &NoEffects
        ),
        Err(Error::Workspace(_))
    ));
    let wrong_manifest = DirectWorkspaceResolverV1::new(
        NativeGitResolverV1::new(),
        Sha256Digest::new(format!("sha256:{}", "b".repeat(64))).unwrap(),
    );
    assert!(matches!(
        remove_workspace_v1(
            &wrong_manifest,
            request(root.path(), "remove", Some(old.clone())),
            &NoEffects
        ),
        Err(Error::Workspace(_))
    ));
    let mut retry = request(root.path(), "remove", Some(old.clone()));
    retry.request_id = RequestIdV1::new(RESPONSE).unwrap();
    let output = remove_workspace_v1(&resolver, retry, &NoEffects).unwrap();
    assert_eq!(output.as_value()["request_id"], OTHER);
    assert_eq!(
        output.as_value()["generated_at"],
        "2023-11-14T22:13:20.100Z"
    );
    assert_eq!(output.as_value()["result"]["workspace_uuid"], old.as_str());
    assert_eq!(output.as_value()["result"]["already_absent"], false);
    assert!(!root.path().join(".podway").exists());
}
#[test]
fn concurrent_fresh_init_after_last_marker_survives_final_removal_and_old_uuid_retry() {
    let root = fixture();
    let old = initialize(root.path());
    let resolver = DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest());
    let fresh = Arc::new(Mutex::new(None));
    let saved = fresh.clone();
    let path = root.path().to_path_buf();
    let result = remove_workspace_with_boundary_observer_v1(
        &resolver,
        request(root.path(), "remove", Some(old.clone())),
        &Clock,
        move |boundary| {
            if boundary == Boundary::AfterLastRemovalMarker {
                let uuid = initialize(&path);
                *saved.lock().unwrap() = Some(uuid);
            }
        },
    );
    assert!(matches!(result, Err(Error::Filesystem(_))));
    let fresh = fresh.lock().unwrap().clone().unwrap();
    assert_ne!(fresh, old);
    let before = files(&root.path().join(".podway"));
    assert!(matches!(
        remove_workspace_v1(
            &resolver,
            request(root.path(), "remove", Some(old)),
            &NoEffects
        ),
        Err(Error::Workspace(_))
    ));
    assert_eq!(files(&root.path().join(".podway")), before);
    let (_, store) = resolver
        .resolve_existing(selector(root.path()), Some(&fresh), deadline())
        .unwrap();
    store.verify_integrity(deadline()).unwrap();
}

#[test]
fn removal_force_is_initial_authority_and_marked_recipe_resumes_without_it() {
    use podway_engine::direct_start::{
        DirectStartCommandV1, DirectStartRequestV1, start_session_v1,
    };
    use podway_protocol::{SessionStartSourceV1, SessionStartV1};
    let root = fixture();
    let old = initialize(root.path());
    let resolver = DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest());
    start_session_v1(
        &resolver,
        DirectStartRequestV1 {
            selector: selector(root.path()),
            expected_workspace_id: None,
            request_id: RequestIdV1::new(RESPONSE).unwrap(),
            key: Some(IdempotencyKeyV1::new("start").unwrap()),
            deadline: deadline(),
            command: DirectStartCommandV1::Start(SessionStartV1 {
                source: SessionStartSourceV1::Preset {
                    preset: "sw-dev-v2".into(),
                },
                expected_procedure_digest: None,
                task_title: "Active removal authorization".into(),
                dry_run: false,
            }),
        },
        &Clock,
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    let before = files(&root.path().join(".podway"));
    assert!(matches!(
        remove_workspace_v1(
            &resolver,
            request(root.path(), "remove", Some(old.clone())),
            &NoEffects
        ),
        Err(Error::ForceRequired)
    ));
    assert_eq!(files(&root.path().join(".podway")), before);
    // Stop immediately after the actual admitted marker, before any deletion.
    let path = root.path().to_path_buf();
    let saved = old.clone();
    let interrupted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut input = request(&path, "remove", Some(saved));
        input.force = true;
        remove_workspace_with_boundary_observer_v1(&resolver, input, &Clock, |boundary| {
            if boundary == Boundary::MarkerPublished {
                panic!("owned marker interruption");
            }
        })
    }));
    assert!(interrupted.is_err());
    // Canonical recipe omits authorization flags and UUID fence; the exact intent owns them.
    let result =
        remove_workspace_v1(&resolver, request(root.path(), "remove", None), &NoEffects).unwrap();
    assert_eq!(result.as_value()["result"]["workspace_uuid"], old.as_str());
    assert!(!root.path().join(".podway").exists());
}
#[test]
fn reset_all_on_missing_workspace_creates_valid_layout_and_one_generation() {
    let root = fixture();
    let resolver = DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest());
    let output = reset_all_v1(
        &resolver,
        request(root.path(), "reset", None),
        &Clock,
        &NativeDirectIdSourceV1,
    )
    .unwrap();
    assert!(validate_daemon_free_document_v1(
        "podway.output/v4",
        output.as_value()
    ));
    assert!(root.path().join(".podway/config.yaml").is_file());
    assert!(root.path().join(".podway/procedures").is_dir());
    assert_eq!(
        std::fs::read_to_string(root.path().join(".podway/.gitignore")).unwrap(),
        "runtime/\n"
    );
    let (_, store) = resolver
        .resolve_existing(selector(root.path()), None, deadline())
        .unwrap();
    store.verify_integrity(deadline()).unwrap();
}
