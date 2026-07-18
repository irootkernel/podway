//! Phase 5 live reset-all runtime wiring contracts.

#![forbid(unsafe_code)]

#[path = "../src/observability.rs"]
#[allow(dead_code)]
mod observability;
#[allow(dead_code)]
mod support_phase4_workspace;

#[path = "../src/registry.rs"]
#[allow(dead_code)]
mod registry_under_test;

use std::{fs, path::Path, process::Command, sync::Arc};

use podway_config::DEFAULT_WORKSPACE_CONFIG_YAML_V1;
use podway_core::{DomainResult, UnixMillis, WorkspaceId};
use podway_daemon::{
    production::compose_dispatcher_v1,
    runtime_workspace::{WorkspaceRuntimeManagerV1, WorkspaceRuntimeObservationV1},
    server::RequestDispatcherV1,
    workspace::ResetMarkerV1,
};
use podway_protocol::{
    ClientInfoV1, CommandNameV1, JobStateV1 as ProtocolJobStateV1, OperationV1, PreconditionsV1,
    RequestEnvelopeInputV1, RequestEnvelopeV1, RequestIdV1, RequestOptionsV1, ResponseEnvelopeV1,
    Rfc3339MillisV1, SliceRequestV1, WorkspaceContextV1, WorktreeSelectorWireV1,
    canonical_reset_all_identity_v1,
};
use podway_service::ServiceRuntimePathsV1;
use podway_store::{
    AdmitOutcomeV1, AdmitRequestV1, CanonicalRequestDigestV1, CommandV1, IdempotencyKeyV1, JobIdV1,
    JobReceiptOrTerminalV1, JobStateV1 as StoreJobStateV1, PersistedTerminalJobStateV1,
    RevisionAttemptItemPreconditionsV1, RevisionV1, SqliteStoreOptionsV1, SqliteStoreV1,
    StoreIdempotencyReadContractV1, StoreReadContractV1, StoreUnavailableReasonV1, WorkerIdV1,
};
use serde_json::json;
use sha2::{Digest as _, Sha256};
#[cfg(unix)]
use std::os::unix::{
    ffi::OsStrExt,
    fs::{MetadataExt, PermissionsExt},
};
use support_phase4_workspace::{git_worktrees, selector};

fn observation() -> WorkspaceRuntimeObservationV1 {
    WorkspaceRuntimeObservationV1::new(
        UnixMillis::new(1_700_000_000_123),
        Rfc3339MillisV1::new("2026-07-16T12:34:56.789Z")
            .expect("fixture registry timestamp must be valid"),
    )
}

fn service_paths(root: &Path) -> ServiceRuntimePathsV1 {
    let application_support = root.join("Application Support");
    fs::create_dir_all(&application_support)
        .expect("fixture application-support parent must exist");
    #[cfg(unix)]
    fs::set_permissions(&application_support, fs::Permissions::from_mode(0o700))
        .expect("fixture application-support parent must be private");
    ServiceRuntimePathsV1::from_directories(
        root.join("LaunchAgents"),
        application_support.join("Podway"),
        root.join("Logs/Podway"),
        root.join("runtime"),
    )
    .expect("fixture service paths must be valid")
}

fn manager(root: &Path) -> WorkspaceRuntimeManagerV1 {
    WorkspaceRuntimeManagerV1::new(
        &service_paths(root),
        SqliteStoreOptionsV1::new(8).expect("fixture inspection options must be valid"),
    )
}

fn registry_store(root: &Path) -> registry_under_test::RegistryStoreV1 {
    registry_under_test::RegistryStoreV1::new(&service_paths(root))
}

fn reset_request(
    root: &Path,
    expected_workspace_uuid: &WorkspaceId,
) -> (RequestEnvelopeV1, SliceRequestV1) {
    #[cfg(unix)]
    let canonical_root = fs::canonicalize(root).expect("fixture worktree root must canonicalize");
    let selector = WorktreeSelectorWireV1::new(
        canonical_root.as_os_str().as_bytes(),
        "reset runtime fixture",
        Some(expected_workspace_uuid.clone()),
    )
    .expect("fixture selector must be valid");
    #[cfg(not(unix))]
    let selector = WorktreeSelectorWireV1::new(
        b"/unsupported",
        "reset runtime fixture",
        Some(expected_workspace_uuid.clone()),
    )
    .expect("fixture selector must be valid");
    let payload = json!({
        "selector": selector,
        "confirmed": true,
        "expected_workspace_uuid": expected_workspace_uuid,
    });
    let envelope = RequestEnvelopeV1::new(RequestEnvelopeInputV1 {
        request_id: RequestIdV1::new("00000000-0000-4000-8000-000000005001")
            .expect("fixture request ID must be valid"),
        client: ClientInfoV1::new("phase5-reset-runtime", "1", 1)
            .expect("fixture client must be valid"),
        operation: OperationV1::Bootstrap,
        command: CommandNameV1::new("workspace.reset_all").expect("fixture command must be valid"),
        workspace: Some(
            WorkspaceContextV1::new("/client/diagnostic", Some(expected_workspace_uuid.clone()))
                .expect("fixture workspace context must be valid"),
        ),
        idempotency_key: Some(
            podway_protocol::IdempotencyKeyV1::new("phase5-live-reset")
                .expect("fixture idempotency key must be valid"),
        ),
        preconditions: PreconditionsV1::default(),
        options: RequestOptionsV1::new(false, 30_000)
            .expect("fixture request options must be valid"),
        payload: payload
            .as_object()
            .expect("fixture payload must be an object")
            .clone(),
    })
    .expect("fixture envelope must be valid");
    let slice = SliceRequestV1::from_envelope(&envelope).expect("fixture reset request must slice");
    (envelope, slice)
}

#[cfg(unix)]
#[test]
fn live_reset_retires_the_old_generation_and_returns_the_target_terminal_receipt() {
    let fixture = git_worktrees();
    fs::set_permissions(
        fixture.main().join(".podway/runtime"),
        fs::Permissions::from_mode(0o700),
    )
    .expect("fixture runtime directory must be private");
    let manager = Arc::new(manager(fixture.temporary_path()));
    let old_scheduler = manager
        .bootstrap(selector(fixture.main()), observation())
        .expect("fixture workspace must bootstrap");
    let old_workspace_uuid = old_scheduler
        .context_snapshot()
        .binding()
        .identity()
        .workspace_uuid()
        .clone();
    manager
        .registered_reset_source_identity(selector(fixture.main()))
        .expect("reset source must resolve through Git and the registry");
    assert!(
        manager
            .discover_reset_marker(selector(fixture.main()))
            .expect("reset marker discovery must resolve through Git")
            .is_none(),
        "a newly bootstrapped workspace must not have a reset marker"
    );
    let dispatcher = compose_dispatcher_v1(
        Arc::clone(&manager),
        WorkerIdV1::new("phase5-reset-runtime").expect("fixture worker ID must be valid"),
    );
    let (request, slice) = reset_request(fixture.main(), &old_workspace_uuid);

    let response = dispatcher.dispatch(&request, &slice);
    let ResponseEnvelopeV1::Output(output) = &response else {
        panic!("reset must return an output envelope: {response:?}");
    };
    let target = output
        .workspace()
        .expect("reset must report its target workspace");
    let completed_job = output
        .job()
        .expect("reset must return its persisted terminal job");
    assert_ne!(target.uuid(), &old_workspace_uuid);
    assert_eq!(target.latest_workspace_sequence(), 1);
    assert_eq!(completed_job.state(), ProtocolJobStateV1::Succeeded);
    assert_eq!(completed_job.sequence(), 1);
    assert_eq!(output.request_id(), request.request_id());
    assert_eq!(output.command(), request.command());
    assert!(
        !fixture.main().join(".podway/runtime/reset.marker").exists(),
        "a successful reset removes its marker only after registry publication"
    );
    assert!(
        manager
            .schedulers()
            .get_active(old_scheduler.key())
            .is_none(),
        "the old scheduler must retire before target activation"
    );
    let old_context = old_scheduler.context_snapshot();
    assert!(matches!(
        old_context
            .store()
            .read_workspace_view(old_context.binding().identity()),
        Err(podway_store::StoreErrorV1::StorageUnavailableV1 {
            reason: StoreUnavailableReasonV1::Recovery
        })
    ));
    let canonical_reset_identity = canonical_reset_all_identity_v1(
        &slice,
        old_context.binding().identity().common_dir_identity(),
        old_context.binding().identity().worktree_admin_identity(),
    )
    .expect("fixture reset identity must canonicalize");
    let expected_reset_digest = CanonicalRequestDigestV1::new(format!(
        "sha256:{:x}",
        Sha256::digest(canonical_reset_identity.as_bytes())
    ))
    .expect("fixture reset digest must be valid");
    let active_target = manager
        .resolve_existing(selector(fixture.main()), Some(target.uuid()), observation())
        .expect("target scheduler must be active after reset");
    assert_eq!(
        active_target
            .context_snapshot()
            .binding()
            .identity()
            .workspace_uuid(),
        target.uuid()
    );

    let target_context = active_target.context_snapshot();
    let stored_job = target_context
        .store()
        .read_job(target_context.binding().identity(), completed_job.id())
        .expect("target Store must remain readable")
        .expect("target terminal job must remain durable");
    assert_eq!(stored_job.state(), StoreJobStateV1::Succeeded);
    assert_eq!(stored_job.job().identity_sequence(), 1);
    assert_eq!(stored_job.job().job_id(), completed_job.id());
    let terminal = stored_job
        .terminal_receipt()
        .expect("succeeded reset must retain its terminal receipt");
    assert_eq!(terminal.job(), stored_job.job());
    assert_eq!(terminal.job().request_digest(), &expected_reset_digest);
    assert_eq!(stored_job.job().request_digest(), &expected_reset_digest);
    assert!(matches!(
        terminal.result(),
        podway_store::codec::PersistedTerminalResultV1::Success(
            podway_store::codec::PersistedDomainResultV1::WorkspaceReset {
                workspace_id,
                revision
            }
        ) if workspace_id == target.uuid() && *revision == podway_store::RevisionV1::ZERO
    ));
    assert!(matches!(
        terminal.job_projection(),
        Some(projection) if projection.state() == PersistedTerminalJobStateV1::Succeeded
    ));
    let replay = target_context
        .store()
        .read_idempotent_outcome(
            target_context.binding().identity(),
            &podway_store::IdempotencyKeyV1::new("phase5-live-reset")
                .expect("fixture idempotency key must be valid"),
            terminal.job().request_digest(),
        )
        .expect("target idempotency replay must remain readable");
    assert_eq!(
        replay,
        Some(AdmitOutcomeV1::Existing(
            JobReceiptOrTerminalV1::TerminalReceipt(terminal.clone())
        ))
    );
    let replay_response = dispatcher.dispatch(&request, &slice);
    let (ResponseEnvelopeV1::Output(original_output), ResponseEnvelopeV1::Output(replayed_output)) =
        (&response, &replay_response)
    else {
        panic!("reset replay must return a success envelope: {replay_response:?}");
    };
    assert_eq!(replayed_output.request_id(), original_output.request_id());
    assert_eq!(replayed_output.command(), original_output.command());
    assert_eq!(replayed_output.workspace(), original_output.workspace());
    assert_eq!(replayed_output.job(), original_output.job());
    assert_eq!(replayed_output.session(), original_output.session());
    assert_eq!(replayed_output.result(), original_output.result());
    assert_eq!(replayed_output.warnings(), original_output.warnings());
}

#[cfg(unix)]
#[test]
fn moved_rebind_retires_the_prior_store_slot_before_reset() {
    let fixture = git_worktrees();
    let linked_runtime = fixture.linked().join(".podway/runtime");
    fs::set_permissions(&linked_runtime, fs::Permissions::from_mode(0o700))
        .expect("linked runtime directory must be private");
    let manager = Arc::new(manager(fixture.temporary_path()));
    let scheduler = manager
        .bootstrap(selector(fixture.linked()), observation())
        .expect("linked workspace must bootstrap");
    let prior = scheduler.context_snapshot();
    let workspace_uuid = prior.binding().identity().workspace_uuid().clone();
    let moved = fixture.temporary_path().join("linked-moved");
    let move_status = Command::new("git")
        .arg("-C")
        .arg(fixture.main())
        .args(["worktree", "move"])
        .arg(fixture.linked())
        .arg(&moved)
        .status()
        .expect("git worktree move must execute");
    assert!(move_status.success(), "git worktree move must succeed");

    let rebound = manager
        .resolve_existing(selector(&moved), Some(&workspace_uuid), observation())
        .expect("moved workspace must rebind");
    assert!(Arc::ptr_eq(&scheduler, &rebound));
    assert!(matches!(
        prior
            .store()
            .read_workspace_view(prior.binding().identity()),
        Err(podway_store::StoreErrorV1::StorageUnavailableV1 {
            reason: StoreUnavailableReasonV1::Recovery
        })
    ));

    let dispatcher = compose_dispatcher_v1(
        Arc::clone(&manager),
        WorkerIdV1::new("phase5-moved-reset").expect("fixture worker ID must be valid"),
    );
    let (request, slice) = reset_request(&moved, &workspace_uuid);
    let response = dispatcher.dispatch(&request, &slice);
    let ResponseEnvelopeV1::Output(output) = response else {
        panic!("moved workspace reset must complete: {response:?}");
    };
    assert_ne!(
        output
            .workspace()
            .expect("reset target workspace must project")
            .uuid(),
        &workspace_uuid
    );
}
#[cfg(unix)]
#[test]
fn stale_marker_predecessor_cannot_authorize_runtime_reset() {
    let fixture = git_worktrees();
    fs::set_permissions(
        fixture.main().join(".podway/runtime"),
        fs::Permissions::from_mode(0o700),
    )
    .expect("fixture runtime directory must be private");
    let manager = Arc::new(manager(fixture.temporary_path()));
    let scheduler = manager
        .bootstrap(selector(fixture.main()), observation())
        .expect("fixture workspace must bootstrap");
    let context = scheduler.context_snapshot();
    let database_path = context.database_path().to_path_buf();
    let before = fs::metadata(&database_path).expect("active database must exist");
    let stale_predecessor = WorkspaceId::new("00000000-0000-4000-8000-000000005089")
        .expect("fixture stale predecessor must be valid");
    let forged = ResetMarkerV1::new(
        JobIdV1::new("00000000-0000-4000-8000-000000005090")
            .expect("fixture reset operation ID must be valid"),
        IdempotencyKeyV1::new("phase5-forged-marker")
            .expect("fixture idempotency key must be valid"),
        CanonicalRequestDigestV1::new(format!("sha256:{}", "f".repeat(64)))
            .expect("fixture reset digest must be valid"),
        stale_predecessor,
        WorkspaceId::new("00000000-0000-4000-8000-000000005091")
            .expect("fixture target workspace UUID must be valid"),
        observation().store_now(),
    );
    let marker_path = fixture.main().join(".podway/runtime/reset.marker");
    fs::write(
        &marker_path,
        forged
            .canonical_bytes()
            .expect("forged marker must encode canonically"),
    )
    .expect("fixture stale marker must be written");
    fs::set_permissions(&marker_path, fs::Permissions::from_mode(0o600))
        .expect("fixture stale marker must be private");

    let dispatcher = compose_dispatcher_v1(
        Arc::clone(&manager),
        WorkerIdV1::new("phase5-forged-marker").expect("fixture worker ID must be valid"),
    );
    let (request, slice) = reset_request(
        fixture.main(),
        context.binding().identity().workspace_uuid(),
    );
    assert!(matches!(
        dispatcher.dispatch(&request, &slice),
        ResponseEnvelopeV1::Error(_)
    ));
    let after = fs::metadata(&database_path).expect("forged marker must not delete the database");
    assert_eq!((after.dev(), after.ino()), (before.dev(), before.ino()));
    assert!(
        marker_path.exists(),
        "a stale predecessor marker must remain evidence rather than authorizing cleanup"
    );
}
#[cfg(unix)]
#[test]
fn registry_predecessor_change_before_marker_publication_cannot_delete_the_source_store() {
    let fixture = git_worktrees();
    fs::set_permissions(
        fixture.main().join(".podway/runtime"),
        fs::Permissions::from_mode(0o700),
    )
    .expect("fixture runtime directory must be private");
    let manager = Arc::new(manager(fixture.temporary_path()));
    let scheduler = manager
        .bootstrap(selector(fixture.main()), observation())
        .expect("fixture workspace must bootstrap");
    let context = scheduler.context_snapshot();
    let previous = context.binding().identity().workspace_uuid().clone();
    let database_path = context.database_path().to_path_buf();
    let before = fs::metadata(&database_path).expect("source database must exist");
    registry_store(fixture.temporary_path())
        .replace_for_reset(
            &previous,
            WorkspaceId::new("00000000-0000-4000-8000-000000005095")
                .expect("fixture changed predecessor must be valid"),
            context.workspace_root().clone(),
            observation().registry_seen_at().clone(),
        )
        .expect("fixture registry predecessor must change");

    let dispatcher = compose_dispatcher_v1(
        Arc::clone(&manager),
        WorkerIdV1::new("phase5-predecessor-change").expect("fixture worker ID must be valid"),
    );
    let (request, slice) = reset_request(fixture.main(), &previous);
    assert!(matches!(
        dispatcher.dispatch(&request, &slice),
        ResponseEnvelopeV1::Error(_)
    ));
    let after = fs::metadata(&database_path).expect("stale reset must preserve source database");
    assert_eq!((after.dev(), after.ino()), (before.dev(), before.ino()));
    assert!(
        !fixture.main().join(".podway/runtime/reset.marker").exists(),
        "a stale predecessor must fail before marker publication"
    );
}
#[cfg(unix)]
#[test]
fn crash_after_target_seed_resumes_from_the_marker_without_recreating_the_target() {
    let fixture = git_worktrees();
    fs::set_permissions(
        fixture.main().join(".podway/runtime"),
        fs::Permissions::from_mode(0o700),
    )
    .expect("fixture runtime directory must be private");
    let manager = Arc::new(manager(fixture.temporary_path()));
    let workspace_selector = selector(fixture.main());
    let candidate = manager
        .resolver()
        .resolve_bootstrap(workspace_selector.clone())
        .expect("fixture workspace must be an unbound candidate");
    manager
        .layout_initializer()
        .initialize_with_config(candidate.worktree(), DEFAULT_WORKSPACE_CONFIG_YAML_V1)
        .expect("fixture layout must initialize");
    let options = SqliteStoreOptionsV1::new(8).expect("fixture Store options must be valid");
    let old_workspace_uuid = candidate.store_identity().workspace_uuid().clone();
    let old_store = SqliteStoreV1::open(
        candidate.database_path(),
        candidate.workspace_root(),
        candidate.store_identity().clone(),
        options.clone(),
        observation().store_now(),
    )
    .expect("fixture old Store must open");
    drop(old_store);
    registry_store(fixture.temporary_path())
        .insert_or_refresh(
            old_workspace_uuid.clone(),
            candidate.workspace_root().clone(),
            observation().registry_seen_at().clone(),
        )
        .expect("fixture old registry entry must publish");

    let reset = manager
        .resolver()
        .resolve_for_reset(workspace_selector.clone())
        .expect("fixture reset worktree must resolve");
    let (request, slice) = reset_request(fixture.main(), &old_workspace_uuid);
    let canonical_reset_identity = canonical_reset_all_identity_v1(
        &slice,
        reset.worktree().identity().common_directory_fingerprint(),
        reset
            .worktree()
            .identity()
            .worktree_administration_fingerprint(),
    )
    .expect("fixture reset identity must canonicalize");
    let reset_digest = CanonicalRequestDigestV1::new(format!(
        "sha256:{:x}",
        Sha256::digest(canonical_reset_identity.as_bytes())
    ))
    .expect("fixture reset digest must be valid");
    let marker = ResetMarkerV1::new(
        JobIdV1::new("00000000-0000-4000-8000-000000005101")
            .expect("fixture reset operation ID must be valid"),
        IdempotencyKeyV1::new("phase5-live-reset").expect("fixture idempotency key must be valid"),
        reset_digest,
        old_workspace_uuid.clone(),
        WorkspaceId::new("00000000-0000-4000-8000-000000005102")
            .expect("fixture target workspace UUID must be valid"),
        observation().store_now(),
    );
    let runtime_path = fixture.main().join(".podway/runtime");
    let marker_path = runtime_path.join("reset.marker");
    fs::write(
        &marker_path,
        marker
            .canonical_bytes()
            .expect("fixture reset marker must encode canonically"),
    )
    .expect("fixture reset marker must be written");
    fs::set_permissions(&marker_path, fs::Permissions::from_mode(0o600))
        .expect("fixture reset marker must be private");
    for name in ["state.sqlite3", "state.sqlite3-wal", "state.sqlite3-shm"] {
        let path = runtime_path.join(name);
        if path.exists() {
            fs::remove_file(path).expect("fixture reset database file must be removed");
        }
    }

    let target_identity = reset.target_identity(marker.target_workspace_uuid().clone());
    let preconditions = RevisionAttemptItemPreconditionsV1::new(None, None, None, None)
        .expect("empty reset preconditions must be valid");
    let seeded = SqliteStoreV1::seed_or_verify_reset_target(
        reset.database_path(),
        reset.workspace_root(),
        target_identity,
        options,
        AdmitRequestV1::new(
            CommandV1::WorkspaceResetAll,
            marker.idempotency_key().clone(),
            marker.operation_id().clone(),
            preconditions,
            marker.request_digest().clone(),
            marker.submitted_at_ms(),
        ),
        DomainResult::WorkspaceReset {
            workspace_id: marker.target_workspace_uuid().clone(),
            revision: RevisionV1::ZERO,
        },
        observation().store_now(),
    )
    .expect("target Store must seed before the simulated crash");
    let seeded_database_identity = {
        let metadata = fs::metadata(reset.database_path())
            .expect("seeded target database must remain inspectable");
        (metadata.dev(), metadata.ino())
    };
    registry_store(fixture.temporary_path())
        .replace_for_reset(
            &old_workspace_uuid,
            marker.target_workspace_uuid().clone(),
            reset.workspace_root().clone(),
            observation().registry_seen_at().clone(),
        )
        .expect("fixture registry target must publish before the simulated crash");

    let dispatcher = compose_dispatcher_v1(
        Arc::clone(&manager),
        WorkerIdV1::new("phase5-crash-resume").expect("fixture worker ID must be valid"),
    );
    let response = dispatcher.dispatch(&request, &slice);
    let ResponseEnvelopeV1::Output(output) = response else {
        panic!("marker recovery must return a success envelope: {response:?}");
    };
    let completed_job = output
        .job()
        .expect("marker recovery must project the existing terminal job");
    assert_eq!(completed_job.id(), seeded.job().job_id());
    assert_eq!(
        output
            .workspace()
            .expect("target workspace must project")
            .uuid(),
        marker.target_workspace_uuid()
    );
    assert!(
        !fixture.main().join(".podway/runtime/reset.marker").exists(),
        "resumed reset removes its marker only after target verification and registry publication"
    );
    let resumed = manager
        .resolve_existing(
            selector(fixture.main()),
            Some(marker.target_workspace_uuid()),
            observation(),
        )
        .expect("resumed target must be activated");
    let context = resumed.context_snapshot();
    let stored_job = context
        .store()
        .read_job(context.binding().identity(), seeded.job().job_id())
        .expect("resumed target Store must remain readable")
        .expect("seeded terminal job must remain durable");
    let stored_receipt = stored_job
        .terminal_receipt()
        .expect("seeded target must retain its exact terminal receipt");
    let expected_receipt = podway_store::PersistedTerminalReceiptV1::from_terminal_receipt(&seeded);
    assert_eq!(stored_receipt.job(), expected_receipt.job());
    assert_eq!(stored_receipt.result(), expected_receipt.result());
    assert_eq!(
        stored_receipt
            .job_projection()
            .expect("new reset seed must retain its terminal job projection")
            .state(),
        PersistedTerminalJobStateV1::Succeeded
    );
    assert!(stored_receipt.session_projection().is_none());
    let metadata = fs::metadata(reset.database_path())
        .expect("resuming an already seeded target must not recreate its database");
    assert_eq!(
        (metadata.dev(), metadata.ino()),
        seeded_database_identity,
        "crash resume must verify the existing database rather than delete and reseed it"
    );
    assert!(
        manager
            .registry()
            .load()
            .expect("resumed registry must remain readable")
            .workspaces()
            .iter()
            .any(|entry| {
                entry.workspace_uuid() == marker.target_workspace_uuid()
                    && entry.last_known_root() == reset.workspace_root()
            })
    );
}
