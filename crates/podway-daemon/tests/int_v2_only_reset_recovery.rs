//! Isolated real-runtime recovery from an unsupported Procedure v1 predecessor.

use super::{int_v2run003_runtime as runtime, support_phase4_workspace};

use std::{
    env, fs,
    os::unix::{ffi::OsStrExt, fs::MetadataExt, fs::PermissionsExt, process::ExitStatusExt},
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::Arc,
};

use podway_core::{
    DomainCommand, JobId, Revision, Sha256Digest, UnixMillis, WorkspaceId, canonicalize_json_v1,
};
use podway_daemon::{
    execution::ResetStoreInspectionV1, runtime_workspace::ResetAllCrashBoundaryV1,
    workspace::ResetMarkerV1,
};
use podway_protocol::{
    PreconditionsV1, RequestIdV1, ResponseEnvelopeV2, WorktreeSelectorWireV1,
    canonical_reset_all_identity_v1,
};
use podway_store::{
    AdmissionSessionIdentityV1, AdmitRequestV1, CancelOutcomeV1, CanonicalExecutionJsonV1,
    CanonicalRequestDigestV1, IdempotencyKeyV1, JobIdV1, RevisionAttemptItemPreconditionsV1,
    SqliteStoreOptionsV1, SqliteStoreV1, StoreContractV1, StoreReadContractV1,
    ValidatedWorkspaceRootV1,
};
use serde_json::{Map, json};
use sha2::{Digest as _, Sha256};

const RESET_CRASH_CHILD_TEST_NAME: &str =
    "int_v2_only_reset_recovery::missing_registry_reset_crash_child";
const RESET_CRASH_WORKTREE_ENV: &str = "PODWAY_RESET_CRASH_WORKTREE";
const RESET_CRASH_SERVICE_ROOT_ENV: &str = "PODWAY_RESET_CRASH_SERVICE_ROOT";
const RESET_CRASH_WORKSPACE_UUID_ENV: &str = "PODWAY_RESET_CRASH_WORKSPACE_UUID";
const RESET_CRASH_BOUNDARY_ENV: &str = "PODWAY_RESET_CRASH_BOUNDARY";

fn selector_with_workspace(path: &Path, workspace: WorkspaceId) -> WorktreeSelectorWireV1 {
    let canonical = fs::canonicalize(path).unwrap();
    WorktreeSelectorWireV1::new(
        canonical.to_string_lossy().as_bytes(),
        canonical.display().to_string(),
        Some(workspace),
    )
    .unwrap()
}

fn reset_request(
    path: &Path,
    workspace: WorkspaceId,
    request_number: u64,
) -> (
    podway_protocol::RequestEnvelopeV1,
    podway_daemon::server::DaemonRequestV1,
) {
    runtime::request(
        request_number,
        "workspace.reset_all",
        &selector_with_workspace(path, workspace.clone()),
        json!({
            "confirmed": true,
            "expected_workspace_uuid": workspace,
        })
        .as_object()
        .unwrap()
        .clone(),
        "missing-registry-reset-crash",
        PreconditionsV1::default(),
    )
}

fn reset_crash_boundary_name(boundary: ResetAllCrashBoundaryV1) -> &'static str {
    match boundary {
        ResetAllCrashBoundaryV1::MarkerCreated => "marker-created",
        ResetAllCrashBoundaryV1::OldDatabaseDeleted => "old-database-deleted",
        ResetAllCrashBoundaryV1::NewTargetDatabaseCreated => "new-target-database-created",
        ResetAllCrashBoundaryV1::RegistryReplaced => "registry-replaced",
        ResetAllCrashBoundaryV1::TargetActivated => "target-activated",
        ResetAllCrashBoundaryV1::MarkerRemoved => "marker-removed",
    }
}

fn parse_reset_crash_boundary(value: &str) -> ResetAllCrashBoundaryV1 {
    match value {
        "marker-created" => ResetAllCrashBoundaryV1::MarkerCreated,
        "old-database-deleted" => ResetAllCrashBoundaryV1::OldDatabaseDeleted,
        "new-target-database-created" => ResetAllCrashBoundaryV1::NewTargetDatabaseCreated,
        "registry-replaced" => ResetAllCrashBoundaryV1::RegistryReplaced,
        "target-activated" => ResetAllCrashBoundaryV1::TargetActivated,
        "marker-removed" => ResetAllCrashBoundaryV1::MarkerRemoved,
        other => panic!("unknown reset crash boundary {other}"),
    }
}

fn run_reset_crash_child(
    worktree: &Path,
    service_root: &Path,
    workspace: &WorkspaceId,
    boundary: ResetAllCrashBoundaryV1,
) -> Output {
    Command::new(env::current_exe().unwrap())
        .arg("--exact")
        .arg(RESET_CRASH_CHILD_TEST_NAME)
        .arg("--nocapture")
        .env(RESET_CRASH_WORKTREE_ENV, worktree)
        .env(RESET_CRASH_SERVICE_ROOT_ENV, service_root)
        .env(RESET_CRASH_WORKSPACE_UUID_ENV, workspace.as_str())
        .env(
            RESET_CRASH_BOUNDARY_ENV,
            reset_crash_boundary_name(boundary),
        )
        .output()
        .expect("reset crash child must start")
}

fn install_v12_reset_marker(worktree: &Path, old_workspace: &WorkspaceId) -> WorkspaceId {
    let database_path = worktree.join(".podway/runtime/state.sqlite3");
    podway_store::test_support::downgrade_to_schema_v12_identity(&database_path).unwrap();
    let legacy = SqliteStoreV1::inspect_reset_workspace_binding(
        &database_path,
        &SqliteStoreOptionsV1::new(8).unwrap(),
    )
    .unwrap()
    .unwrap();
    let request = reset_request(worktree, old_workspace.clone(), 95_021);
    let canonical = canonical_reset_all_identity_v1(
        request.1.legacy().unwrap(),
        legacy.identity().common_dir_identity(),
        legacy.identity().worktree_admin_identity(),
    )
    .unwrap();
    let digest =
        CanonicalRequestDigestV1::new(format!("sha256:{:x}", Sha256::digest(canonical.as_bytes())))
            .unwrap();
    let target_workspace = WorkspaceId::new("00000000-0000-4000-8000-000000009521").unwrap();
    let marker = ResetMarkerV1::new_with_response_request_id(
        JobIdV1::new("00000000-0000-4000-8000-000000009520").unwrap(),
        IdempotencyKeyV1::new("missing-registry-reset-crash").unwrap(),
        digest.clone(),
        old_workspace.clone(),
        target_workspace.clone(),
        UnixMillis::new(1_700_000_000_123),
        RequestIdV1::new("00000000-0000-4000-8000-000000009522").unwrap(),
    );
    let marker_path = worktree.join(".podway/runtime/reset.marker");
    fs::write(&marker_path, marker.canonical_bytes().unwrap()).unwrap();
    fs::set_permissions(&marker_path, fs::Permissions::from_mode(0o600)).unwrap();
    target_workspace
}

#[test]
fn workspace_repair_restores_a_missing_global_registry_entry() {
    let fixture = support_phase4_workspace::git_worktrees();
    runtime::make_runtime_private(fixture.main());
    let manager = Arc::new(runtime::manager(fixture.temporary_path()));
    let initialized = manager
        .bootstrap(
            support_phase4_workspace::selector(fixture.main()),
            runtime::observation(),
        )
        .unwrap();
    let workspace_uuid = initialized
        .context_snapshot()
        .binding()
        .identity()
        .workspace_uuid()
        .clone();
    let registry_path = manager.registry().registry_path().to_path_buf();
    drop(initialized);
    fs::remove_file(&registry_path).unwrap();

    let dispatcher = runtime::dispatcher(Arc::clone(&manager), "repair-missing-registry");
    let repair = runtime::request(
        95_000,
        "workspace.repair",
        &runtime::selector(fixture.main()),
        Map::new(),
        "unused-repair-key",
        PreconditionsV1::default(),
    );
    let response = runtime::dispatch(&dispatcher, &repair);
    let ResponseEnvelopeV2::OutputV2(output) = response else {
        panic!("workspace repair must restore a missing registry entry: {response:?}");
    };
    assert_eq!(output.command().as_str(), "workspace.repair");
    assert_eq!(output.workspace().unwrap().uuid(), &workspace_uuid);
    assert_eq!(output.result()["changed"], true);
    assert_eq!(
        output.result()["changes"],
        json!(["registry.last_known_root"])
    );
    let restored = manager.registry().load().unwrap();
    let entry = restored.lookup(&workspace_uuid).unwrap();
    assert_eq!(
        entry.last_known_root().unix_bytes(),
        fs::canonicalize(fixture.main())
            .unwrap()
            .as_os_str()
            .as_encoded_bytes()
    );
}

#[test]
fn existing_v12_workspace_migrates_from_filesystem_to_path_identity() {
    let fixture = support_phase4_workspace::git_worktrees();
    runtime::make_runtime_private(fixture.main());
    let manager = Arc::new(runtime::manager(fixture.temporary_path()));
    let initialized = manager
        .bootstrap(
            support_phase4_workspace::selector(fixture.main()),
            runtime::observation(),
        )
        .unwrap();
    let workspace_uuid = initialized
        .context_snapshot()
        .binding()
        .identity()
        .workspace_uuid()
        .clone();
    let database_path = fixture.main().join(".podway/runtime/state.sqlite3");
    drop(initialized);
    drop(manager);

    podway_store::test_support::downgrade_to_schema_v12_identity(&database_path).unwrap();

    let manager = runtime::manager(fixture.temporary_path());
    let resolved = manager
        .resolve_existing(
            support_phase4_workspace::selector(fixture.main()),
            Some(&workspace_uuid),
            runtime::observation(),
        )
        .expect("v12 identity must migrate at the same canonical worktree root");
    assert_eq!(
        resolved
            .context_snapshot()
            .binding()
            .identity()
            .workspace_uuid(),
        &workspace_uuid
    );
    drop(resolved);

    let binding = SqliteStoreV1::inspect_workspace_binding(
        database_path,
        &SqliteStoreOptionsV1::new(8).unwrap(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(binding.schema_version(), 13);
}

#[test]
fn confirmed_reset_all_recovers_a_store_when_the_registry_entry_is_missing() {
    let fixture = support_phase4_workspace::git_worktrees();
    runtime::make_runtime_private(fixture.main());
    let manager = Arc::new(runtime::manager(fixture.temporary_path()));
    let initialized = manager
        .bootstrap(
            support_phase4_workspace::selector(fixture.main()),
            runtime::observation(),
        )
        .unwrap();
    let old_workspace = initialized
        .context_snapshot()
        .binding()
        .identity()
        .workspace_uuid()
        .clone();
    let registry_path = manager.registry().registry_path().to_path_buf();
    drop(initialized);
    fs::remove_file(&registry_path).unwrap();

    let dispatcher = runtime::dispatcher(Arc::clone(&manager), "reset-missing-registry");
    let reset = runtime::request(
        95_010,
        "workspace.reset_all",
        &selector_with_workspace(fixture.main(), old_workspace.clone()),
        json!({
            "confirmed": true,
            "expected_workspace_uuid": old_workspace,
        })
        .as_object()
        .unwrap()
        .clone(),
        "reset-missing-registry-confirmed",
        PreconditionsV1::default(),
    );
    let response = runtime::dispatch(&dispatcher, &reset);
    let ResponseEnvelopeV2::OutputV2(output) = response else {
        panic!("confirmed reset-all must recover a missing registry entry: {response:?}");
    };
    let new_workspace = output.workspace().unwrap().uuid().clone();
    assert_ne!(new_workspace, old_workspace);
    let registry = manager.registry().load().unwrap();
    assert!(registry.lookup(&old_workspace).is_none());
    assert_eq!(
        registry
            .lookup(&new_workspace)
            .expect("the reset target must be registered")
            .last_known_root()
            .unix_bytes(),
        fs::canonicalize(fixture.main())
            .unwrap()
            .as_os_str()
            .as_encoded_bytes()
    );
}

#[test]
fn missing_registry_reset_crash_child() {
    let Some(worktree) = env::var_os(RESET_CRASH_WORKTREE_ENV) else {
        return;
    };
    let service_root = PathBuf::from(env::var_os(RESET_CRASH_SERVICE_ROOT_ENV).unwrap());
    let workspace = WorkspaceId::new(env::var(RESET_CRASH_WORKSPACE_UUID_ENV).unwrap()).unwrap();
    let boundary = parse_reset_crash_boundary(&env::var(RESET_CRASH_BOUNDARY_ENV).unwrap());
    let manager = Arc::new(runtime::manager_with_reset_crash_boundary(
        &service_root,
        boundary,
    ));
    let dispatcher = runtime::dispatcher(manager, "missing-registry-reset-crash-child");
    let request = reset_request(Path::new(&worktree), workspace, 95_021);
    let response = runtime::dispatch(&dispatcher, &request);
    panic!("configured reset crash boundary returned without aborting: {response:?}");
}

#[test]
fn missing_registry_reset_crash_boundaries_converge_on_cold_replay() {
    for boundary in [
        ResetAllCrashBoundaryV1::MarkerCreated,
        ResetAllCrashBoundaryV1::OldDatabaseDeleted,
        ResetAllCrashBoundaryV1::NewTargetDatabaseCreated,
        ResetAllCrashBoundaryV1::RegistryReplaced,
        ResetAllCrashBoundaryV1::TargetActivated,
        ResetAllCrashBoundaryV1::MarkerRemoved,
    ] {
        let fixture = support_phase4_workspace::git_worktrees();
        runtime::make_runtime_private(fixture.main());
        let manager = runtime::manager(fixture.temporary_path());
        let initialized = manager
            .bootstrap(
                support_phase4_workspace::selector(fixture.main()),
                runtime::observation(),
            )
            .unwrap();
        let old_workspace = initialized
            .context_snapshot()
            .binding()
            .identity()
            .workspace_uuid()
            .clone();
        let registry_path = manager.registry().registry_path().to_path_buf();
        drop(initialized);
        drop(manager);
        fs::remove_file(&registry_path).unwrap();

        let output = run_reset_crash_child(
            fixture.main(),
            fixture.temporary_path(),
            &old_workspace,
            boundary,
        );
        assert_eq!(
            output.status.signal(),
            Some(6),
            "{} child must abort: stdout={} stderr={}",
            reset_crash_boundary_name(boundary),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );

        let marker_path = fixture.main().join(".podway/runtime/reset.marker");
        let marker = fs::read(&marker_path)
            .ok()
            .map(|bytes| ResetMarkerV1::decode_canonical(&bytes).unwrap());
        if let Some(marker) = marker.as_ref() {
            assert_eq!(marker.previous_workspace_uuid(), &old_workspace);
            assert_ne!(marker.target_workspace_uuid(), &old_workspace);
        }
        let database_path = fixture.main().join(".podway/runtime/state.sqlite3");
        let target_workspace = match boundary {
            ResetAllCrashBoundaryV1::MarkerCreated => {
                let binding = SqliteStoreV1::inspect_reset_workspace_binding(
                    &database_path,
                    &SqliteStoreOptionsV1::new(8).unwrap(),
                )
                .unwrap()
                .unwrap();
                assert_eq!(binding.identity().workspace_uuid(), &old_workspace);
                marker.as_ref().unwrap().target_workspace_uuid().clone()
            }
            ResetAllCrashBoundaryV1::OldDatabaseDeleted => {
                assert!(!database_path.exists());
                marker.as_ref().unwrap().target_workspace_uuid().clone()
            }
            ResetAllCrashBoundaryV1::NewTargetDatabaseCreated
            | ResetAllCrashBoundaryV1::RegistryReplaced
            | ResetAllCrashBoundaryV1::TargetActivated
            | ResetAllCrashBoundaryV1::MarkerRemoved => {
                let binding = SqliteStoreV1::inspect_workspace_binding(
                    &database_path,
                    &SqliteStoreOptionsV1::new(8).unwrap(),
                )
                .unwrap()
                .unwrap();
                if let Some(marker) = marker.as_ref() {
                    assert_eq!(
                        binding.identity().workspace_uuid(),
                        marker.target_workspace_uuid()
                    );
                }
                binding.identity().workspace_uuid().clone()
            }
        };

        if boundary == ResetAllCrashBoundaryV1::MarkerRemoved {
            assert!(!marker_path.exists());
        } else {
            assert!(marker_path.exists());
        }

        let recovery_manager = Arc::new(runtime::manager(fixture.temporary_path()));
        let dispatcher = runtime::dispatcher(
            Arc::clone(&recovery_manager),
            "missing-registry-reset-recovery",
        );
        let recovered = runtime::dispatch(
            &dispatcher,
            &reset_request(fixture.main(), old_workspace.clone(), 95_021),
        );
        let ResponseEnvelopeV2::OutputV2(output) = recovered else {
            panic!("reset crash replay must converge: {recovered:?}");
        };
        let new_workspace = output.workspace().unwrap().uuid();
        assert_eq!(new_workspace, &target_workspace);
        assert!(!marker_path.exists());
        let registry = recovery_manager.registry().load().unwrap();
        assert!(registry.lookup(&old_workspace).is_none());
        assert!(registry.lookup(new_workspace).is_some());
    }
}

#[test]
fn v12_reset_marker_and_completed_receipt_replay_across_path_identity_migration() {
    for crash_boundary in [
        None,
        Some(ResetAllCrashBoundaryV1::OldDatabaseDeleted),
        Some(ResetAllCrashBoundaryV1::NewTargetDatabaseCreated),
        Some(ResetAllCrashBoundaryV1::RegistryReplaced),
    ] {
        let fixture = support_phase4_workspace::git_worktrees();
        runtime::make_runtime_private(fixture.main());
        let manager = runtime::manager(fixture.temporary_path());
        let initialized = manager
            .bootstrap(
                support_phase4_workspace::selector(fixture.main()),
                runtime::observation(),
            )
            .unwrap();
        let old_workspace = initialized
            .context_snapshot()
            .binding()
            .identity()
            .workspace_uuid()
            .clone();
        drop(initialized);
        drop(manager);

        let target_workspace = install_v12_reset_marker(fixture.main(), &old_workspace);
        if let Some(boundary) = crash_boundary {
            let output = run_reset_crash_child(
                fixture.main(),
                fixture.temporary_path(),
                &old_workspace,
                boundary,
            );
            assert_eq!(
                output.status.signal(),
                Some(6),
                "{} child must abort: stdout={} stderr={}",
                reset_crash_boundary_name(boundary),
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );
        }

        let recovery_manager = Arc::new(runtime::manager(fixture.temporary_path()));
        let dispatcher =
            runtime::dispatcher(Arc::clone(&recovery_manager), "v12-reset-marker-recovery");
        let request = reset_request(fixture.main(), old_workspace.clone(), 95_021);
        let recovered = runtime::dispatch_after_cold_reopen(&dispatcher, &request);
        let ResponseEnvelopeV2::OutputV2(output) = recovered else {
            panic!(
                "v12 reset marker must replay after migration at {crash_boundary:?}: {recovered:?}"
            );
        };
        assert_eq!(output.workspace().unwrap().uuid(), &target_workspace);
        assert!(!fixture.main().join(".podway/runtime/reset.marker").exists());

        drop(dispatcher);
        drop(recovery_manager);
        let replay_manager = Arc::new(runtime::manager(fixture.temporary_path()));
        let replay_dispatcher = runtime::dispatcher(replay_manager, "v12-reset-receipt-replay");
        let replayed = runtime::dispatch_after_cold_reopen(&replay_dispatcher, &request);
        let ResponseEnvelopeV2::OutputV2(output) = replayed else {
            panic!("completed v12 reset receipt must replay after migration: {replayed:?}");
        };
        assert_eq!(output.workspace().unwrap().uuid(), &target_workspace);
    }
}

#[test]
fn bootstrap_rejects_a_new_workspace_identity_at_an_already_registered_root() {
    let fixture = support_phase4_workspace::git_worktrees();
    runtime::make_runtime_private(fixture.main());
    let manager = Arc::new(runtime::manager(fixture.temporary_path()));
    let initialized = manager
        .bootstrap(
            support_phase4_workspace::selector(fixture.main()),
            runtime::observation(),
        )
        .unwrap();
    let expected_registered_workspace = initialized
        .context_snapshot()
        .binding()
        .identity()
        .workspace_uuid()
        .clone();
    let database_path = fixture.main().join(".podway/runtime/state.sqlite3");
    drop(initialized);
    drop(manager);

    for path in [
        database_path.clone(),
        database_path.with_extension("sqlite3-wal"),
        database_path.with_extension("sqlite3-shm"),
    ] {
        match fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => panic!("fixture database removal failed: {error}"),
        }
    }

    let manager = runtime::manager(fixture.temporary_path());
    let error = match manager.bootstrap(
        support_phase4_workspace::selector(fixture.main()),
        runtime::observation(),
    ) {
        Ok(_) => panic!("bootstrap must not create a second workspace UUID at the same root"),
        Err(error) => error,
    };
    assert!(matches!(
        error,
        podway_daemon::runtime_workspace::WorkspaceRuntimeErrorV1::Registry(
            podway_daemon::registry::RegistryErrorV1::WorkspaceRootOccupied {
                registered_workspace_uuid,
                ..
            }
        ) if registered_workspace_uuid == expected_registered_workspace
    ));
    assert!(
        !database_path.exists(),
        "root ownership must be rejected before Store creation"
    );
}

#[test]
fn reset_rejects_a_store_bound_to_another_exact_root() {
    let fixture = support_phase4_workspace::git_worktrees();
    runtime::make_runtime_private(fixture.main());
    let manager = runtime::manager(fixture.temporary_path());
    let initialized = manager
        .bootstrap(
            support_phase4_workspace::selector(fixture.main()),
            runtime::observation(),
        )
        .unwrap();
    let database_path = fixture.main().join(".podway/runtime/state.sqlite3");
    drop(initialized);
    drop(manager);

    podway_store::test_support::detach_workspace_root(&database_path).unwrap();

    let manager = runtime::manager(fixture.temporary_path());
    let error = match manager
        .registered_reset_source_authority(support_phase4_workspace::selector(fixture.main()))
    {
        Ok(_) => panic!("a copied Store root must not authorize destructive reset"),
        Err(error) => error,
    };
    assert!(matches!(
        error,
        podway_daemon::runtime_workspace::WorkspaceRuntimeErrorV1::ResetSourceAmbiguous
    ));
}

#[test]
fn reset_rejects_a_local_store_uuid_registered_at_another_root_before_deletion() {
    let fixture = support_phase4_workspace::git_worktrees();
    runtime::make_runtime_private(fixture.main());
    let manager = runtime::manager(fixture.temporary_path());
    let initialized = manager
        .bootstrap(
            support_phase4_workspace::selector(fixture.main()),
            runtime::observation(),
        )
        .unwrap();
    let workspace_uuid = initialized
        .context_snapshot()
        .binding()
        .identity()
        .workspace_uuid()
        .clone();
    let registry_path = manager.registry().registry_path().to_path_buf();
    let database_path = fixture.main().join(".podway/runtime/state.sqlite3");
    drop(initialized);
    drop(manager);

    let database_before = fs::read(&database_path).expect("predecessor database bytes");
    let mut registry: serde_json::Value =
        serde_json::from_slice(&fs::read(&registry_path).unwrap()).unwrap();
    let entry = registry["workspaces"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|entry| entry["workspace_uuid"] == workspace_uuid.as_str())
        .unwrap();
    let linked_root = fs::canonicalize(fixture.linked()).unwrap();
    entry["last_known_root"] = serde_json::Value::String(
        ValidatedWorkspaceRootV1::from_unix_bytes(linked_root.as_os_str().as_bytes().to_vec())
            .unwrap()
            .as_encoded()
            .to_owned(),
    );
    fs::write(&registry_path, canonicalize_json_v1(&registry).unwrap()).unwrap();

    let manager = Arc::new(runtime::manager(fixture.temporary_path()));
    let error = match manager
        .registered_reset_source_authority(support_phase4_workspace::selector(fixture.main()))
    {
        Ok(_) => panic!("a predecessor UUID registered at another root must fail closed"),
        Err(error) => error,
    };
    assert!(
        matches!(
            &error,
            podway_daemon::runtime_workspace::WorkspaceRuntimeErrorV1::Registry(
                podway_daemon::registry::RegistryErrorV1::WorkspaceRootConflict { .. }
            )
        ),
        "unexpected copied-UUID reset error: {error:?}"
    );
    let dispatcher = runtime::dispatcher(Arc::clone(&manager), "copied-uuid-reset");
    let request = runtime::request(
        95_013,
        "workspace.reset_all",
        &selector_with_workspace(fixture.main(), workspace_uuid.clone()),
        json!({
            "confirmed": true,
            "expected_workspace_uuid": workspace_uuid,
        })
        .as_object()
        .unwrap()
        .clone(),
        "copied-uuid-reset-rejected",
        PreconditionsV1::default(),
    );
    let response = runtime::dispatch(&dispatcher, &request);
    let ResponseEnvelopeV2::Error(error) = response else {
        panic!("copied UUID reset must fail closed: {response:?}");
    };
    assert_eq!(error.code().as_str(), "WORKSPACE_ID_CONFLICT");
    assert_eq!(
        fs::read(&database_path).expect("rejected reset database bytes"),
        database_before
    );
    assert!(!fixture.main().join(".podway/runtime/reset.marker").exists());
}

#[test]
fn reset_rechecks_initially_absent_marker_uuids_after_marker_publication() {
    for conflict_target in [false, true] {
        let fixture = support_phase4_workspace::git_worktrees();
        runtime::make_runtime_private(fixture.main());
        let manager = runtime::manager(fixture.temporary_path());
        let initialized = manager
            .bootstrap(
                support_phase4_workspace::selector(fixture.main()),
                runtime::observation(),
            )
            .unwrap();
        let old_workspace = initialized
            .context_snapshot()
            .binding()
            .identity()
            .workspace_uuid()
            .clone();
        let registry_path = manager.registry().registry_path().to_path_buf();
        let database_path = fixture.main().join(".podway/runtime/state.sqlite3");
        let database_before = fs::metadata(&database_path).unwrap();
        drop(initialized);
        drop(manager);
        fs::remove_file(&registry_path).unwrap();

        let conflicting_root_path = fs::canonicalize(fixture.linked()).unwrap();
        let conflicting_root = ValidatedWorkspaceRootV1::from_unix_bytes(
            conflicting_root_path.as_os_str().as_bytes().to_vec(),
        )
        .unwrap();
        let hook_registry_path = registry_path.clone();
        let manager = Arc::new(runtime::manager_with_reset_post_marker_hook(
            fixture.temporary_path(),
            move |marker| {
                let workspace_uuid = if conflict_target {
                    marker.target_workspace_uuid().clone()
                } else {
                    marker.previous_workspace_uuid().clone()
                };
                let registry = json!({
                    "schema": "podway.registry/v1",
                    "workspaces": [{
                        "last_known_root": conflicting_root.as_encoded(),
                        "last_seen_at": "2027-01-15T08:00:01.000Z",
                        "workspace_uuid": workspace_uuid,
                    }],
                });
                fs::write(
                    &hook_registry_path,
                    canonicalize_json_v1(&registry).unwrap(),
                )
                .unwrap();
                fs::set_permissions(&hook_registry_path, fs::Permissions::from_mode(0o600))
                    .unwrap();
            },
        ));
        let dispatcher = runtime::dispatcher(Arc::clone(&manager), "post-marker-uuid-conflict");
        let response = runtime::dispatch(
            &dispatcher,
            &reset_request(fixture.main(), old_workspace.clone(), 95_014),
        );
        let ResponseEnvelopeV2::Error(error) = response else {
            panic!("a post-marker UUID registration must fail closed: {response:?}");
        };
        assert_eq!(error.code().as_str(), "WORKSPACE_ID_CONFLICT");
        let database_after = fs::metadata(&database_path).unwrap();
        assert_eq!(database_after.dev(), database_before.dev());
        assert_eq!(database_after.ino(), database_before.ino());
        let binding = SqliteStoreV1::inspect_reset_workspace_binding(
            &database_path,
            &SqliteStoreOptionsV1::new(8).unwrap(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(binding.identity().workspace_uuid(), &old_workspace);
        assert!(fixture.main().join(".podway/runtime/reset.marker").exists());
    }
}

#[test]
fn reset_marker_resume_accepts_an_exact_root_with_detached_git_identity() {
    let fixture = support_phase4_workspace::git_worktrees();
    runtime::make_runtime_private(fixture.main());
    let manager = runtime::manager(fixture.temporary_path());
    let initialized = manager
        .bootstrap(
            support_phase4_workspace::selector(fixture.main()),
            runtime::observation(),
        )
        .unwrap();
    let old_workspace = initialized
        .context_snapshot()
        .binding()
        .identity()
        .workspace_uuid()
        .clone();
    let database_path = fixture.main().join(".podway/runtime/state.sqlite3");
    drop(initialized);
    drop(manager);

    podway_store::test_support::detach_git_identity(&database_path).unwrap();
    let marker = ResetMarkerV1::new_with_response_request_id(
        JobIdV1::new("00000000-0000-4000-8000-000000009910").unwrap(),
        IdempotencyKeyV1::new("detached-marker-resume").unwrap(),
        CanonicalRequestDigestV1::new(format!("sha256:{}", "9".repeat(64))).unwrap(),
        old_workspace.clone(),
        WorkspaceId::new("00000000-0000-4000-8000-000000009911").unwrap(),
        UnixMillis::new(1_700_000_000_123),
        RequestIdV1::new("00000000-0000-4000-8000-000000009912").unwrap(),
    );
    let marker_path = fixture.main().join(".podway/runtime/reset.marker");
    fs::write(&marker_path, marker.canonical_bytes().unwrap()).unwrap();
    fs::set_permissions(&marker_path, fs::Permissions::from_mode(0o600)).unwrap();

    let manager = runtime::manager(fixture.temporary_path());
    let authority = manager
        .registered_reset_source_authority(support_phase4_workspace::selector(fixture.main()))
        .expect("the durable marker and exact Store root must authorize reset recovery");
    assert_eq!(authority.registry_previous_workspace_uuid(), &old_workspace);
    assert!(matches!(
        authority.store_inspection(),
        ResetStoreInspectionV1::GitIdentityDetached
    ));
}

#[test]
fn reset_all_recovers_when_full_store_openability_fails_internal_codec() {
    let fixture = support_phase4_workspace::git_worktrees();
    runtime::make_runtime_private(fixture.main());
    let manager = Arc::new(runtime::manager(fixture.temporary_path()));
    let initialized = manager
        .bootstrap(
            support_phase4_workspace::selector(fixture.main()),
            runtime::observation(),
        )
        .unwrap();
    let binding = initialized.context_snapshot().binding().clone();
    let old_workspace = binding.identity().workspace_uuid().clone();
    let database_path = fixture.main().join(".podway/runtime/state.sqlite3");
    drop(initialized);
    drop(manager);

    let store = SqliteStoreV1::open(
        &database_path,
        binding.last_validated_root(),
        binding.identity().clone(),
        SqliteStoreOptionsV1::new(8).unwrap(),
        UnixMillis::new(10),
    )
    .unwrap();
    let job_id = JobId::new("00000000-0000-4000-8000-000000009920").unwrap();
    let execution = CanonicalExecutionJsonV1::new(
        canonicalize_json_v1(&json!({
            "command": "session.start",
            "execution_version": 6,
            "procedure": {"canonical": true}
        }))
        .unwrap(),
    )
    .unwrap();
    store
        .admit(
            binding.identity(),
            AdmitRequestV1::new_with_canonical_execution(
                DomainCommand::SessionStart,
                IdempotencyKeyV1::new("reset-openability-codec").unwrap(),
                job_id.clone(),
                RevisionAttemptItemPreconditionsV1::new(None, None, None, None).unwrap(),
                Sha256Digest::new(format!("sha256:{}", "a".repeat(64))).unwrap(),
                UnixMillis::new(11),
                execution,
            )
            .with_procedure_v2_execution()
            .with_session_identity(AdmissionSessionIdentityV1::Absent),
        )
        .unwrap();
    assert!(matches!(
        store
            .cancel_before_claim(
                binding.identity(),
                job_id.clone(),
                Revision::new(1),
                UnixMillis::new(12),
            )
            .unwrap(),
        CancelOutcomeV1::Cancelled(_)
    ));
    drop(store);
    podway_store::test_support::rewrite_terminal_as_noncanonical(&database_path, &job_id).unwrap();

    let manager = Arc::new(runtime::manager(fixture.temporary_path()));
    let authority = manager
        .registered_reset_source_authority(support_phase4_workspace::selector(fixture.main()))
        .expect("the exact binding must remain reset authority");
    assert!(matches!(
        authority.store_inspection(),
        ResetStoreInspectionV1::Unreadable(podway_store::StoreErrorV1::StorageIntegrityV1 {
            check: podway_store::StoreIntegrityCheckV1::InternalCodec
        })
    ));

    let dispatcher = runtime::dispatcher(Arc::clone(&manager), "codec-reset-recovery");
    let reset = runtime::request(
        95_020,
        "workspace.reset_all",
        &selector_with_workspace(fixture.main(), old_workspace.clone()),
        json!({
            "confirmed": true,
            "expected_workspace_uuid": old_workspace,
        })
        .as_object()
        .unwrap()
        .clone(),
        "codec-reset-confirmed",
        PreconditionsV1::default(),
    );
    let response = runtime::dispatch(&dispatcher, &reset);
    let ResponseEnvelopeV2::OutputV2(output) = response else {
        panic!(
            "confirmed reset-all must replace a Store that fails full openability: {response:?}"
        );
    };
    let new_workspace = output.workspace().unwrap().uuid().clone();
    assert_eq!(output.command().as_str(), "workspace.reset_all");
    assert_ne!(new_workspace, old_workspace);
    assert!(
        manager
            .registry()
            .load()
            .unwrap()
            .lookup(&old_workspace)
            .is_none()
    );
    drop(dispatcher);
    drop(manager);

    let manager = Arc::new(runtime::manager(fixture.temporary_path()));
    let reopened = manager
        .resolve_existing(
            support_phase4_workspace::selector(fixture.main()),
            Some(&new_workspace),
            runtime::observation(),
        )
        .expect("reset replacement must cold-reopen");
    let binding = reopened.context_snapshot().binding().clone();
    assert_eq!(binding.identity().workspace_uuid(), &new_workspace);
    let authority = manager
        .registered_reset_source_authority(support_phase4_workspace::selector(fixture.main()))
        .expect("replacement binding must remain readable reset authority");
    assert!(matches!(
        authority.store_inspection(),
        ResetStoreInspectionV1::Readable
    ));
    drop(reopened);
    drop(manager);

    let store = SqliteStoreV1::open(
        &database_path,
        binding.last_validated_root(),
        binding.identity().clone(),
        SqliteStoreOptionsV1::new(8).unwrap(),
        UnixMillis::new(13),
    )
    .expect("replacement Store must open normally");
    assert!(
        store
            .read_job(binding.identity(), &job_id)
            .unwrap()
            .is_none(),
        "reset replacement must not retain the corrupted predecessor job"
    );
}

#[test]
fn v2cut_legacy_schema_v3_rejects_then_confirmed_reset_all_replaces_and_cold_reopens() {
    let fixture = support_phase4_workspace::git_worktrees();
    runtime::make_runtime_private(fixture.main());
    let manager = Arc::new(runtime::manager(fixture.temporary_path()));
    let initialized = manager
        .bootstrap(
            support_phase4_workspace::selector(fixture.main()),
            runtime::observation(),
        )
        .unwrap();
    let old_workspace = initialized
        .context_snapshot()
        .binding()
        .identity()
        .workspace_uuid()
        .clone();
    let registry_path = manager.registry().registry_path().to_path_buf();
    drop(initialized);
    drop(manager);

    let database_path = fixture.main().join(".podway/runtime/state.sqlite3");
    podway_store::test_support::downgrade_to_schema_v3_with_legacy_snapshot(
        &database_path,
        "podway.procedure/v1",
    )
    .unwrap();
    podway_store::test_support::detach_git_identity(&database_path).unwrap();

    let stale_workspace = WorkspaceId::new("00000000-0000-4000-8000-000000009901").unwrap();
    let mut registry: serde_json::Value =
        serde_json::from_slice(&fs::read(&registry_path).unwrap()).unwrap();
    let workspaces = registry["workspaces"].as_array_mut().unwrap();
    let current = workspaces
        .iter()
        .find(|entry| entry["workspace_uuid"] == old_workspace.as_str())
        .unwrap();
    workspaces.push(json!({
        "last_known_root": current["last_known_root"],
        "last_seen_at": "2026-07-14T12:34:56.789Z",
        "workspace_uuid": stale_workspace,
    }));
    workspaces.sort_by(|left, right| {
        left["workspace_uuid"]
            .as_str()
            .cmp(&right["workspace_uuid"].as_str())
    });
    fs::write(&registry_path, canonicalize_json_v1(&registry).unwrap()).unwrap();

    let selector = selector_with_workspace(fixture.main(), old_workspace.clone());
    let manager = Arc::new(runtime::manager(fixture.temporary_path()));
    let authority = manager
        .registered_reset_source_authority(support_phase4_workspace::selector(fixture.main()))
        .expect("the local Store binding must select the reset predecessor");
    assert_eq!(authority.registry_previous_workspace_uuid(), &old_workspace);
    assert_eq!(authority.registry_root_workspace_uuids().len(), 2);
    let dispatcher = runtime::dispatcher(Arc::clone(&manager), "v2cut-reset-recovery");
    let rejected = runtime::request(
        95_002,
        "workspace.init",
        &selector,
        Map::new(),
        "v2cut-reset-rejected-open",
        PreconditionsV1::default(),
    );
    let rejected = runtime::dispatch(&dispatcher, &rejected);
    let ResponseEnvelopeV2::Error(error) = &rejected else {
        panic!("legacy Procedure v1 state must fail closed before reset: {rejected:?}");
    };
    assert_eq!(error.code().as_str(), "LEGACY_PROCEDURE_STATE_UNSUPPORTED");

    let reset = runtime::request(
        95_003,
        "workspace.reset_all",
        &selector,
        json!({
            "confirmed": true,
            "expected_workspace_uuid": old_workspace,
        })
        .as_object()
        .unwrap()
        .clone(),
        "v2cut-reset-confirmed",
        PreconditionsV1::default(),
    );
    let reset = runtime::dispatch(&dispatcher, &reset);
    let ResponseEnvelopeV2::OutputV2(reset_output) = &reset else {
        panic!("confirmed reset-all must replace the unsupported legacy store: {reset:?}");
    };
    let new_workspace = reset_output.workspace().unwrap().uuid().clone();
    let reset_job_id = reset_output.job().unwrap().id().clone();
    let reset_terminal_response = serde_json::to_value(reset_output).unwrap();
    assert_ne!(new_workspace, old_workspace);
    assert_eq!(reset_output.command().as_str(), "workspace.reset_all");
    let registry = manager.registry().load().unwrap();
    assert!(registry.lookup(&old_workspace).is_none());
    assert!(registry.lookup(&stale_workspace).is_none());
    assert_eq!(
        registry
            .lookup(&new_workspace)
            .expect("the reset target must be the sole owner of the worktree root")
            .last_known_root()
            .unix_bytes(),
        fs::canonicalize(fixture.main())
            .unwrap()
            .as_os_str()
            .as_encoded_bytes()
    );
    drop(dispatcher);
    drop(manager);

    let selector = selector_with_workspace(fixture.main(), new_workspace.clone());
    let manager = Arc::new(runtime::manager(fixture.temporary_path()));
    let reopened = manager
        .resolve_existing(
            support_phase4_workspace::selector(fixture.main()),
            Some(&new_workspace),
            runtime::observation(),
        )
        .unwrap();
    assert_eq!(
        reopened
            .context_snapshot()
            .binding()
            .identity()
            .workspace_uuid(),
        &new_workspace
    );
    let dispatcher = runtime::dispatcher(Arc::clone(&manager), "v2cut-reset-cold-reopen");
    let show = runtime::request(
        95_004,
        "workspace.show",
        &selector,
        Map::new(),
        "",
        PreconditionsV1::default(),
    );
    let ResponseEnvelopeV2::OutputV2(shown) = runtime::dispatch(&dispatcher, &show) else {
        panic!("replacement workspace must cold-reopen through the production dispatcher");
    };
    assert_eq!(shown.workspace().unwrap().uuid(), &new_workspace);

    let status = runtime::request(
        95_005,
        "job.status",
        &selector,
        json!({"job_id": reset_job_id}).as_object().unwrap().clone(),
        "",
        PreconditionsV1::default(),
    );
    let status = runtime::v2_result(runtime::dispatch(&dispatcher, &status), "job.status");
    assert_eq!(status["job"], reset_terminal_response);

    let list = runtime::request(
        95_006,
        "job.list",
        &selector,
        Map::new(),
        "",
        PreconditionsV1::default(),
    );
    let list = runtime::v2_result(runtime::dispatch(&dispatcher, &list), "job.list");
    let listed_reset = list["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|job| job["id"] == reset_job_id.as_str())
        .expect("the unfiltered job list must include the reset job");
    assert_eq!(listed_reset["terminal_response"], reset_terminal_response);

    let lookup = runtime::request(
        95_007,
        "job.lookup",
        &selector,
        json!({"idempotency_key": "v2cut-reset-confirmed"})
            .as_object()
            .unwrap()
            .clone(),
        "",
        PreconditionsV1::default(),
    );
    let lookup = runtime::v2_result(runtime::dispatch(&dispatcher, &lookup), "job.lookup");
    assert_eq!(lookup["found"], true);
    assert_eq!(lookup["job"]["id"], reset_job_id.as_str());
    assert_eq!(lookup["job"]["terminal_response"], reset_terminal_response);
    drop(dispatcher);
    drop(manager);

    assert!(
        podway_store::SqliteStoreV1::inspect_workspace_binding(
            database_path,
            &podway_store::SqliteStoreOptionsV1::new(8).unwrap(),
        )
        .unwrap()
        .is_some()
    );
}
