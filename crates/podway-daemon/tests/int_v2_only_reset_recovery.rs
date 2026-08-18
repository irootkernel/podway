//! Isolated real-runtime recovery from an unsupported Procedure v1 predecessor.

use super::{int_v2run003_runtime as runtime, support_phase4_workspace};

use std::{fs, os::unix::fs::PermissionsExt, path::Path, sync::Arc};

use podway_core::{
    DomainCommand, JobId, Revision, Sha256Digest, UnixMillis, WorkspaceId, canonicalize_json_v1,
};
use podway_daemon::{execution::ResetStoreInspectionV1, workspace::ResetMarkerV1};
use podway_protocol::{PreconditionsV1, RequestIdV1, ResponseEnvelopeV2, WorktreeSelectorWireV1};
use podway_store::{
    AdmissionSessionIdentityV1, AdmitRequestV1, CancelOutcomeV1, CanonicalExecutionJsonV1,
    CanonicalRequestDigestV1, IdempotencyKeyV1, JobIdV1, RevisionAttemptItemPreconditionsV1,
    SqliteStoreOptionsV1, SqliteStoreV1, StoreContractV1, StoreReadContractV1,
};
use serde_json::{Map, json};

fn selector_with_workspace(path: &Path, workspace: WorkspaceId) -> WorktreeSelectorWireV1 {
    let canonical = fs::canonicalize(path).unwrap();
    WorktreeSelectorWireV1::new(
        canonical.to_string_lossy().as_bytes(),
        canonical.display().to_string(),
        Some(workspace),
    )
    .unwrap()
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
