//! Phase 5 live reset-all runtime wiring contracts.

#![forbid(unsafe_code)]

use crate::{registry_under_test, support_phase4_workspace};

use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, mpsc},
    thread,
    time::{Duration, Instant},
};

use podway_config::DEFAULT_WORKSPACE_CONFIG_YAML_V1;
use podway_core::{DomainResult, UnixMillis, WorkspaceId};
use podway_daemon::{
    production::compose_dispatcher_v1,
    runtime_workspace::{
        ResetAllCrashBoundaryV1, WorkspaceRuntimeErrorV1, WorkspaceRuntimeManagerV1,
        WorkspaceRuntimeObservationV1,
    },
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
    JobListQueryV1, JobReceiptOrTerminalV1, JobStateV1 as StoreJobStateV1,
    PersistedTerminalJobStateV1, RevisionAttemptItemPreconditionsV1, RevisionV1,
    SqliteStoreOptionsV1, SqliteStoreV1, StoreIdempotencyReadContractV1, StoreReadContractV1,
    StoreUnavailableReasonV1, WorkerIdV1,
};
use serde_json::json;
use sha2::{Digest as _, Sha256};
#[cfg(unix)]
use std::os::unix::{
    ffi::OsStrExt,
    fs::{MetadataExt, PermissionsExt},
    process::ExitStatusExt,
};
use support_phase4_workspace::{git_worktrees, selector};
fn fixture_runtime_directory(root: &Path) -> PathBuf {
    let root = fs::canonicalize(root).expect("fixture root must canonicalize");
    #[cfg(unix)]
    let digest = Sha256::digest(root.as_os_str().as_bytes());
    #[cfg(not(unix))]
    let digest = Sha256::digest(root.to_string_lossy().as_bytes());
    let digest = format!("{digest:x}");
    std::env::temp_dir().join(format!("pdr-{}", &digest[..16]))
}
#[cfg(unix)]
fn terminal_receipt_rows(database_path: &Path) -> String {
    let immutable_database = format!("file:{}?immutable=1", database_path.display());
    let output = Command::new("sqlite3")
        .args(["-batch", "-noheader", "-separator", "\u{1f}"])
        .arg(immutable_database)
        .arg(
            "SELECT 'jobs', job_id, workspace_sequence, idempotency_key, request_digest, \
             state, submitted_at_ms, claimed_at_ms, finished_at_ms, hex(terminal_response_json) \
             FROM jobs \
             UNION ALL \
             SELECT 'idempotency_records', idempotency_key, request_digest, job_id, scope_kind, \
             scope_session_id, created_at_ms, updated_at_ms, NULL, hex(terminal_response_json) \
             FROM idempotency_records \
             ORDER BY 1",
        )
        .output()
        .expect("sqlite3 must inspect the immutable published replacement database");
    assert!(
        output.status.success(),
        "sqlite3 must read terminal receipt rows: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("sqlite3 terminal receipt rows must be valid UTF-8")
}

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
        fixture_runtime_directory(root),
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
    reset_request_with_key(root, expected_workspace_uuid, 5_001, "phase5-live-reset")
}

fn reset_request_with_key(
    root: &Path,
    expected_workspace_uuid: &WorkspaceId,
    request_number: u64,
    idempotency_key: &str,
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
        request_id: RequestIdV1::new(format!("00000000-0000-4000-8000-{request_number:012x}"))
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
            podway_protocol::IdempotencyKeyV1::new(idempotency_key)
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
fn reset_lookup_request(
    root: &Path,
    expected_workspace_uuid: &WorkspaceId,
    marker: &ResetMarkerV1,
) -> (RequestEnvelopeV1, SliceRequestV1) {
    #[cfg(unix)]
    let canonical_root = fs::canonicalize(root).expect("fixture worktree root must canonicalize");
    let selector = WorktreeSelectorWireV1::new(
        canonical_root.as_os_str().as_bytes(),
        "reset reconciliation fixture",
        Some(expected_workspace_uuid.clone()),
    )
    .expect("fixture selector must be valid");
    let payload = json!({
        "selector": selector,
        "idempotency_key": marker.idempotency_key().as_str(),
    });
    let envelope = RequestEnvelopeV1::new(RequestEnvelopeInputV1 {
        request_id: RequestIdV1::new("00000000-0000-4000-8000-000000005019")
            .expect("fixture request ID must be valid"),
        client: ClientInfoV1::new("phase5-reset-runtime", "1", 1)
            .expect("fixture client must be valid"),
        operation: OperationV1::Query,
        command: CommandNameV1::new("job.lookup").expect("fixture command must be valid"),
        workspace: Some(
            WorkspaceContextV1::new("/client/diagnostic", Some(expected_workspace_uuid.clone()))
                .expect("fixture workspace context must be valid"),
        ),
        idempotency_key: None,
        preconditions: PreconditionsV1::default(),
        options: RequestOptionsV1::new(false, 0).expect("query options must be valid"),
        payload: payload
            .as_object()
            .expect("fixture payload must be an object")
            .clone(),
    })
    .expect("fixture envelope must be valid");
    let slice = SliceRequestV1::from_envelope(&envelope).expect("fixture lookup must slice");
    (envelope, slice)
}

fn workspace_mutation_request(
    root: &Path,
    workspace_uuid: &WorkspaceId,
    request_number: u64,
    command: &str,
    idempotency_key: &str,
    preconditions: PreconditionsV1,
    mut payload: serde_json::Value,
) -> (RequestEnvelopeV1, SliceRequestV1) {
    #[cfg(unix)]
    let canonical_root = fs::canonicalize(root).expect("fixture worktree root must canonicalize");
    #[cfg(unix)]
    let selector = WorktreeSelectorWireV1::new(
        canonical_root.as_os_str().as_bytes(),
        "reset runtime fixture",
        Some(workspace_uuid.clone()),
    )
    .expect("fixture selector must be valid");
    #[cfg(not(unix))]
    let selector = WorktreeSelectorWireV1::new(
        b"/unsupported",
        "reset runtime fixture",
        Some(workspace_uuid.clone()),
    )
    .expect("fixture selector must be valid");
    payload
        .as_object_mut()
        .expect("fixture payload must be an object")
        .insert(
            "selector".to_owned(),
            serde_json::to_value(selector).expect("fixture selector must serialize"),
        );
    let envelope = RequestEnvelopeV1::new(RequestEnvelopeInputV1 {
        request_id: RequestIdV1::new(format!("00000000-0000-4000-8000-{request_number:012x}"))
            .expect("fixture request ID must be valid"),
        client: ClientInfoV1::new("phase5-reset-runtime", "1", 1)
            .expect("fixture client must be valid"),
        operation: OperationV1::Mutate,
        command: CommandNameV1::new(command).expect("fixture command must be valid"),
        workspace: Some(
            WorkspaceContextV1::new("/client/diagnostic", Some(workspace_uuid.clone()))
                .expect("fixture workspace context must be valid"),
        ),
        idempotency_key: Some(
            podway_protocol::IdempotencyKeyV1::new(idempotency_key)
                .expect("fixture idempotency key must be valid"),
        ),
        preconditions,
        options: RequestOptionsV1::new(false, 30_000)
            .expect("fixture request options must be valid"),
        payload: payload
            .as_object()
            .expect("fixture payload must be an object")
            .clone(),
    })
    .expect("fixture envelope must be valid");
    let slice =
        SliceRequestV1::from_envelope(&envelope).expect("fixture mutation request must slice");
    (envelope, slice)
}

#[cfg(unix)]
#[test]
fn pac_044_reset_all_destroys_history_recreates_a_mutable_workspace_and_replays_idempotently() {
    if let (Some(runtime_root), Some(worktree), Some(workspace_uuid_output)) = (
        std::env::var_os("PODWAY_PAC044_SEED_RUNTIME_ROOT"),
        std::env::var_os("PODWAY_PAC044_SEED_WORKTREE"),
        std::env::var_os("PODWAY_PAC044_SEED_UUID_OUTPUT"),
    ) {
        let runtime_root = PathBuf::from(runtime_root);
        let worktree = PathBuf::from(worktree);
        let manager = Arc::new(manager(&runtime_root));
        let scheduler = manager
            .bootstrap(selector(&worktree), observation())
            .expect("seed child workspace must bootstrap");
        let workspace_uuid = scheduler
            .context_snapshot()
            .binding()
            .identity()
            .workspace_uuid()
            .clone();
        fs::write(
            PathBuf::from(workspace_uuid_output),
            workspace_uuid.as_str(),
        )
        .expect("seed child must publish its workspace UUID");
        let dispatcher = compose_dispatcher_v1(
            manager,
            WorkerIdV1::new("phase5-reset-runtime-seed-child")
                .expect("fixture worker ID must be valid"),
        );
        let (seed_history_envelope, seed_history) = workspace_mutation_request(
            &worktree,
            &workspace_uuid,
            5_002,
            "session.start",
            "phase5-reset-history",
            PreconditionsV1::default(),
            json!({
                "preset": "sw-dev",
                "task_title": "PAC-044 history that reset must destroy",
            }),
        );
        assert!(
            matches!(
                dispatcher.dispatch(&seed_history_envelope, &seed_history),
                ResponseEnvelopeV1::Output(_)
            ),
            "the source workspace must persist session history before reset-all"
        );
        return;
    }

    let fixture = git_worktrees();
    fs::set_permissions(
        fixture.main().join(".podway/runtime"),
        fs::Permissions::from_mode(0o700),
    )
    .expect("fixture runtime directory must be private");
    let workspace_uuid_output = fixture.temporary_path().join("pac044-workspace-uuid");
    let seed_output = Command::new(std::env::current_exe().expect("current test binary"))
        .arg("--exact")
        .arg("int_phase5_reset_runtime::pac_044_reset_all_destroys_history_recreates_a_mutable_workspace_and_replays_idempotently")
        .arg("--nocapture")
        .env(
            "PODWAY_PAC044_SEED_RUNTIME_ROOT",
            fixture.temporary_path(),
        )
        .env("PODWAY_PAC044_SEED_WORKTREE", fixture.main())
        .env("PODWAY_PAC044_SEED_UUID_OUTPUT", &workspace_uuid_output)
        .output()
        .expect("seed child must launch");
    assert!(
        seed_output.status.success(),
        "seed child must bootstrap and persist task history through the public daemon path: stdout={:?} stderr={:?}",
        String::from_utf8_lossy(&seed_output.stdout),
        String::from_utf8_lossy(&seed_output.stderr),
    );
    let old_workspace_uuid = WorkspaceId::new(
        fs::read_to_string(&workspace_uuid_output)
            .expect("seed child workspace UUID must be readable"),
    )
    .expect("seed child workspace UUID must be valid");

    let manager = Arc::new(manager(fixture.temporary_path()));
    let old_scheduler = manager
        .resolve_existing(
            selector(fixture.main()),
            Some(&old_workspace_uuid),
            observation(),
        )
        .expect("seeded source scheduler must reopen");
    let old_scheduler_key = old_scheduler.key().clone();
    let old_context = old_scheduler.context_snapshot();
    assert!(
        old_context
            .store()
            .read_session_aggregate(old_context.binding().identity())
            .expect("seeded source session must be readable")
            .is_some(),
        "reset-all must have actual source history to destroy"
    );
    let old_common_dir_identity = old_context
        .binding()
        .identity()
        .common_dir_identity()
        .clone();
    let old_worktree_admin_identity = old_context
        .binding()
        .identity()
        .worktree_admin_identity()
        .clone();
    drop(old_context);
    drop(old_scheduler);

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
            .get_active(&old_scheduler_key)
            .is_none(),
        "the old scheduler must retire before target activation"
    );
    let canonical_reset_identity = canonical_reset_all_identity_v1(
        &slice,
        &old_common_dir_identity,
        &old_worktree_admin_identity,
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
    assert_eq!(
        target_context.config().schema,
        "podway.workspace/v1",
        "reset-all must recreate a valid workspace configuration"
    );
    assert!(
        target_context
            .store()
            .read_session_aggregate(target_context.binding().identity())
            .expect("fresh target Store must be readable")
            .is_none(),
        "the new workspace must not retain seeded source session history"
    );
    let target_identity = target_context.binding().identity();
    let sequence_before_stale_request = target_context
        .store()
        .read_workspace_view(target_identity)
        .expect("fresh target workspace view must be readable")
        .latest_workspace_sequence();
    let jobs_before_stale_request = target_context
        .store()
        .list_jobs(
            target_identity,
            JobListQueryV1::new(100).expect("stale request job query must be valid"),
        )
        .expect("fresh target jobs must be readable");
    let (stale_start_envelope, stale_start) = workspace_mutation_request(
        fixture.main(),
        &old_workspace_uuid,
        5_005,
        "session.start",
        "phase5-reset-stale-workspace",
        PreconditionsV1::default(),
        json!({
            "preset": "sw-dev",
            "task_title": "must not enter replacement workspace",
        }),
    );
    let ResponseEnvelopeV1::Error(stale_error) =
        dispatcher.dispatch(&stale_start_envelope, &stale_start)
    else {
        panic!("the replaced workspace identity must reject stale mutation admission");
    };
    assert_eq!(stale_error.code().as_str(), "WORKSPACE_UUID_MISMATCH");
    assert_eq!(stale_error.exit_code().get(), 4);
    assert!(!stale_error.retryable());
    assert_eq!(
        stale_error.details(),
        &serde_json::Map::from_iter([
            (
                "schema".to_owned(),
                json!("podway.workspace-uuid-mismatch-details/v1"),
            ),
            (
                "expected_workspace_uuid".to_owned(),
                json!(old_workspace_uuid.as_str()),
            ),
            (
                "actual_workspace_uuid".to_owned(),
                json!(target.uuid().as_str()),
            ),
            ("admission".to_owned(), json!({"admitted": false})),
        ])
    );
    assert!(
        target_context
            .store()
            .read_session_aggregate(target_identity)
            .expect("target session must remain readable")
            .is_none(),
        "a stale workspace request must not create a target session"
    );
    assert_eq!(
        target_context
            .store()
            .list_jobs(
                target_identity,
                JobListQueryV1::new(100).expect("stale request job query must remain valid"),
            )
            .expect("target jobs must remain readable"),
        jobs_before_stale_request,
        "a stale workspace request must not admit a target job"
    );
    assert_eq!(
        target_context
            .store()
            .read_workspace_view(target_identity)
            .expect("target workspace view must remain readable")
            .latest_workspace_sequence(),
        sequence_before_stale_request,
        "a stale workspace request must not consume target sequence"
    );
    let target_database_before_stale_reset =
        fs::metadata(target_context.database_path()).expect("target database must exist");
    let (stale_reset_envelope, stale_reset) = reset_request_with_key(
        fixture.main(),
        &old_workspace_uuid,
        5_006,
        "phase5-stale-reset-identity",
    );
    let ResponseEnvelopeV1::Error(stale_reset_error) =
        dispatcher.dispatch(&stale_reset_envelope, &stale_reset)
    else {
        panic!("a fresh-key reset against the replaced UUID must fail identity validation");
    };
    assert_eq!(stale_reset_error.code().as_str(), "WORKSPACE_UUID_MISMATCH");
    assert_eq!(stale_reset_error.exit_code().get(), 4);
    assert!(!stale_reset_error.retryable());
    assert_eq!(
        stale_reset_error.details(),
        &serde_json::Map::from_iter([
            (
                "schema".to_owned(),
                json!("podway.workspace-uuid-mismatch-details/v1"),
            ),
            (
                "expected_workspace_uuid".to_owned(),
                json!(old_workspace_uuid.as_str()),
            ),
            (
                "actual_workspace_uuid".to_owned(),
                json!(target.uuid().as_str()),
            ),
            ("admission".to_owned(), json!({"admitted": false})),
        ])
    );
    let target_database_after_stale_reset =
        fs::metadata(target_context.database_path()).expect("target database must remain");
    assert_eq!(
        (
            target_database_after_stale_reset.dev(),
            target_database_after_stale_reset.ino(),
        ),
        (
            target_database_before_stale_reset.dev(),
            target_database_before_stale_reset.ino(),
        ),
        "a stale reset must not replace the target Store"
    );
    assert_eq!(
        target_context
            .store()
            .list_jobs(
                target_identity,
                JobListQueryV1::new(100).expect("stale reset job query must be valid"),
            )
            .expect("target jobs must remain readable"),
        jobs_before_stale_request,
        "a stale reset must not admit a target job"
    );
    assert_eq!(
        target_context
            .store()
            .read_workspace_view(target_identity)
            .expect("target workspace view must remain readable")
            .latest_workspace_sequence(),
        sequence_before_stale_request,
        "a stale reset must not consume target sequence"
    );
    assert!(
        !fixture.main().join(".podway/runtime/reset.marker").exists(),
        "a stale reset must fail before marker publication"
    );
    let (fresh_start_envelope, fresh_start) = workspace_mutation_request(
        fixture.main(),
        target.uuid(),
        5_003,
        "session.start",
        "phase5-reset-fresh-start",
        PreconditionsV1::default(),
        json!({
            "preset": "sw-dev",
            "task_title": "PAC-044 fresh session",
        }),
    );
    assert!(
        matches!(
            dispatcher.dispatch(&fresh_start_envelope, &fresh_start),
            ResponseEnvelopeV1::Output(_)
        ),
        "a fresh session start must succeed after reset-all"
    );
    let fresh_session = target_context
        .store()
        .read_session_aggregate(target_context.binding().identity())
        .expect("fresh session must be readable")
        .expect("fresh session must exist");
    let fresh_attempt = fresh_session
        .active_attempt_id()
        .expect("fresh session must have an active attempt")
        .clone();
    let goal_revision = fresh_session
        .attempts()
        .iter()
        .find(|attempt| attempt.attempt_id() == &fresh_attempt)
        .expect("fresh active attempt must exist")
        .item_slots()
        .iter()
        .find(|slot| slot.item_id().as_str() == "goal")
        .expect("fresh goal item must exist")
        .revision();
    let (fresh_mutation_envelope, fresh_mutation) = workspace_mutation_request(
        fixture.main(),
        target.uuid(),
        5_004,
        "item.set",
        "phase5-reset-fresh-mutation",
        PreconditionsV1::new(
            Some(fresh_session.session_id().clone()),
            None,
            Some(fresh_attempt.clone()),
            Some(goal_revision),
            None,
            None,
        )
        .expect("fresh item preconditions must be valid"),
        json!({
            "item_id": "goal",
            "value": "PAC-044 post-reset mutation",
        }),
    );
    let mutation_response = dispatcher.dispatch(&fresh_mutation_envelope, &fresh_mutation);
    let ResponseEnvelopeV1::Output(mutation_output) = &mutation_response else {
        panic!("a fresh session mutation must succeed after reset-all: {mutation_response:?}");
    };
    let mutation_job = mutation_output
        .job()
        .expect("mutation must project its terminal job");
    assert_eq!(mutation_job.state(), ProtocolJobStateV1::Succeeded);
    let reread_session = target_context
        .store()
        .read_session_aggregate(target_context.binding().identity())
        .expect("post-reset mutation must be reread from the durable Store")
        .expect("post-reset session must remain durable");
    let reread_goal = reread_session
        .attempts()
        .iter()
        .find(|attempt| attempt.attempt_id() == &fresh_attempt)
        .expect("post-reset active attempt must remain durable")
        .item_slots()
        .iter()
        .find(|slot| slot.item_id().as_str() == "goal")
        .expect("post-reset goal item must remain durable");
    assert_eq!(
        reread_goal.revision(),
        goal_revision
            .checked_next()
            .expect("goal revision must advance")
    );
    assert_eq!(
        reread_goal
            .value()
            .and_then(podway_core::ItemValueV1::as_text),
        Some("PAC-044 post-reset mutation")
    );
    let stored_mutation_job = target_context
        .store()
        .read_job(target_context.binding().identity(), mutation_job.id())
        .expect("post-reset mutation job must be reread from the durable Store")
        .expect("post-reset mutation job must remain durable");
    assert_eq!(stored_mutation_job.state(), StoreJobStateV1::Succeeded);
    assert_eq!(
        stored_mutation_job.job().identity_sequence(),
        mutation_job.sequence()
    );
    assert_eq!(stored_mutation_job.job().job_id(), mutation_job.id());
    let mutation_terminal = stored_mutation_job
        .terminal_receipt()
        .expect("post-reset mutation must retain a terminal receipt");
    assert_eq!(mutation_terminal.job(), stored_mutation_job.job());
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
    assert!(matches!(
        terminal.lookup_command(),
        Some(podway_store::codec::PersistedDomainCommandV1::WorkspaceResetAll)
    ));
    let reset_response_context = terminal
        .response_context()
        .expect("reset receipt must retain its full response context");
    assert_eq!(
        reset_response_context.request_id(),
        request.request_id().as_str()
    );
    assert_eq!(reset_response_context.command(), "workspace.reset_all");
    assert_eq!(reset_response_context.workspace_uuid(), target.uuid());
    assert_eq!(reset_response_context.workspace_root(), target.root());
    assert_eq!(reset_response_context.workspace_sequence(), 1);
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
    let pruned = Command::new("sqlite3")
        .arg(target_context.database_path())
        .arg(format!(
            "DELETE FROM jobs WHERE job_id = '{}';",
            completed_job.id().as_str()
        ))
        .output()
        .expect("sqlite3 must create the receipt-only post-pruning shape");
    assert!(
        pruned.status.success(),
        "receipt-only pruning fixture must succeed: {}",
        String::from_utf8_lossy(&pruned.stderr)
    );
    let target_uuid = target.uuid().clone();
    drop(target_context);
    drop(active_target);
    drop(dispatcher);
    drop(manager);

    let restarted_manager = Arc::new(self::manager(fixture.temporary_path()));
    let restart_deadline = Instant::now() + Duration::from_secs(5);
    let active_target = loop {
        match restarted_manager.resolve_existing(
            selector(fixture.main()),
            Some(&target_uuid),
            observation(),
        ) {
            Ok(scheduler) => break scheduler,
            Err(WorkspaceRuntimeErrorV1::MaintenanceInProgress)
                if Instant::now() < restart_deadline =>
            {
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => panic!("receipt-only reset target must survive manager restart: {error}"),
        }
    };
    let target_context = active_target.context_snapshot();
    let target_identity = target_context.binding().identity();
    assert!(
        target_context
            .store()
            .read_job(target_identity, completed_job.id())
            .expect("pruned reset job lookup must remain readable")
            .is_none(),
        "the replay below must use the receipt-only path"
    );
    let sequence_before_replay = target_context
        .store()
        .read_workspace_view(target_context.binding().identity())
        .expect("target workspace view must remain readable")
        .latest_workspace_sequence();
    let (lock_held_tx, lock_held_rx) = mpsc::channel();
    let (lock_release_tx, lock_release_rx) = mpsc::channel();
    let (lock_finished_tx, lock_finished_rx) = mpsc::channel();
    let held_scheduler = Arc::clone(&active_target);
    let lock_holder = thread::spawn(move || {
        held_scheduler.with_serialized(|_| {
            lock_held_tx
                .send(())
                .expect("held scheduler lock must signal acquisition");
            lock_release_rx
                .recv()
                .expect("held scheduler lock must receive release");
        });
        lock_finished_tx
            .send(())
            .expect("held scheduler lock must signal release");
    });
    lock_held_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("reset replay must contend with an acquired scheduler lock");
    let (replay_complete, replay_result) = mpsc::channel();
    let replay_manager = Arc::clone(&restarted_manager);
    let replay_request = request.clone();
    let replay_slice = slice.clone();
    let replay_thread = thread::spawn(move || {
        let replay_dispatcher = compose_dispatcher_v1(
            replay_manager,
            WorkerIdV1::new("phase5-reset-runtime-replay")
                .expect("fixture replay worker ID must be valid"),
        );
        replay_complete
            .send(replay_dispatcher.dispatch(&replay_request, &replay_slice))
            .expect("reset replay result receiver must remain available");
    });
    assert!(
        replay_result
            .recv_timeout(Duration::from_millis(250))
            .is_err(),
        "reset replay must not complete while the target scheduler lock is held"
    );
    lock_release_tx
        .send(())
        .expect("held scheduler lock must receive release");
    lock_finished_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("held scheduler lock must release");
    lock_holder
        .join()
        .expect("held scheduler lock thread must not panic");
    let replay_response = replay_result
        .recv_timeout(Duration::from_secs(5))
        .expect("reset replay must complete after scheduler lock release");
    replay_thread
        .join()
        .expect("reset replay thread must not panic");
    let (ResponseEnvelopeV1::Output(_), ResponseEnvelopeV1::Output(_)) =
        (&response, &replay_response)
    else {
        panic!("reset replay must return a success envelope: {replay_response:?}");
    };
    assert_eq!(
        replay_response, response,
        "receipt-only reset replay after restart must reproduce the complete original output envelope"
    );
    assert_eq!(
        target_context
            .store()
            .read_workspace_view(target_identity)
            .expect("target workspace must remain readable after receipt-only replay")
            .latest_workspace_sequence(),
        sequence_before_replay,
        "receipt-only reset replay must not mutate the target workspace",
    );
}

#[cfg(unix)]
#[test]
fn dropped_manager_generation_does_not_block_recreated_manager_reset() {
    let fixture = git_worktrees();
    let paths = service_paths(fixture.temporary_path());
    let options = SqliteStoreOptionsV1::new(8).expect("fixture SQLite options must be valid");
    let first_manager = WorkspaceRuntimeManagerV1::new(&paths, options.clone());
    let first_scheduler = first_manager
        .bootstrap(selector(fixture.main()), observation())
        .expect("first manager must activate the source scheduler");
    let previous_workspace_uuid = first_scheduler
        .context_snapshot()
        .binding()
        .identity()
        .workspace_uuid()
        .clone();
    drop(first_scheduler);
    drop(first_manager);

    let manager = Arc::new(WorkspaceRuntimeManagerV1::new(&paths, options));
    let dispatcher = compose_dispatcher_v1(
        Arc::clone(&manager),
        WorkerIdV1::new("phase5-dropped-manager-reset").expect("fixture worker ID must be valid"),
    );
    let (request, slice) = reset_request(fixture.main(), &previous_workspace_uuid);
    let ResponseEnvelopeV1::Output(output) = dispatcher.dispatch(&request, &slice) else {
        panic!("recreated manager must reset after the prior generation is dropped");
    };
    assert_ne!(
        output
            .workspace()
            .expect("reset target workspace must project")
            .uuid(),
        &previous_workspace_uuid
    );
}
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
    let ResponseEnvelopeV1::Error(error) = dispatcher.dispatch(&request, &slice) else {
        panic!("forged reset marker must return the public maintenance error");
    };
    assert_eq!(error.code().as_str(), "WORKSPACE_MAINTENANCE");
    assert_eq!(
        error.message(),
        "Workspace maintenance temporarily blocks mutation admission."
    );
    assert_eq!(error.exit_code().get(), 4);
    assert!(error.retryable());
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
    let ResponseEnvelopeV1::Error(error) = dispatcher.dispatch(&request, &slice) else {
        panic!("stale registry predecessor must return the public identity-conflict error");
    };
    assert_eq!(error.code().as_str(), "WORKSPACE_UUID_MISMATCH");
    assert_eq!(
        error.message(),
        "The workspace UUID differs from the expected identity."
    );
    assert_eq!(error.exit_code().get(), 4);
    assert!(!error.retryable());
    assert_eq!(
        error.details(),
        &serde_json::Map::from_iter([
            (
                "schema".to_owned(),
                json!("podway.workspace-uuid-mismatch-details/v1"),
            ),
            (
                "expected_workspace_uuid".to_owned(),
                json!("00000000-0000-4000-8000-000000005095")
            ),
            ("actual_workspace_uuid".to_owned(), json!(previous.as_str()),),
            ("admission".to_owned(), json!({"admitted": false})),
        ])
    );
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
#[cfg(unix)]
const RESET_CRASH_REPORT_ENV: &str = "PODWAY_PHASE5_RESET_CRASH_REPORT";
#[cfg(unix)]
const RESET_CRASH_CHILD_TEST: &str =
    "int_phase5_reset_runtime::reset_all_crash_child_aborts_at_configured_boundary";
#[cfg(unix)]
const RESET_CRASH_BOUNDARY_ENV: &str = "PODWAY_PHASE5_RESET_CRASH_BOUNDARY";

#[cfg(unix)]
fn selected_reset_crash_boundary() -> ResetAllCrashBoundaryV1 {
    match std::env::var(RESET_CRASH_BOUNDARY_ENV).as_deref() {
        Ok("MarkerCreated") => ResetAllCrashBoundaryV1::MarkerCreated,
        Ok("OldDatabaseDeleted") => ResetAllCrashBoundaryV1::OldDatabaseDeleted,
        Ok("NewTargetDatabaseCreated") => ResetAllCrashBoundaryV1::NewTargetDatabaseCreated,
        value => panic!("invalid isolated reset crash boundary: {value:?}"),
    }
}

#[cfg(unix)]
#[test]
fn reset_all_crash_child_aborts_at_configured_boundary() {
    let report_path = match (
        std::env::var(RESET_CRASH_REPORT_ENV),
        std::env::var(RESET_CRASH_BOUNDARY_ENV),
    ) {
        (Ok(report_path), Ok(_)) => report_path,
        (Err(_), Err(_)) => return,
        state => panic!("reset crash child mode requires both marker inputs, got {state:?}"),
    };
    let fixture = git_worktrees();
    fs::set_permissions(
        fixture.main().join(".podway/runtime"),
        fs::Permissions::from_mode(0o700),
    )
    .expect("child runtime directory must be private");
    let manager = Arc::new(
        WorkspaceRuntimeManagerV1::with_reset_crash_boundary_for_tests(
            &service_paths(fixture.temporary_path()),
            SqliteStoreOptionsV1::new(8).expect("fixture inspection options must be valid"),
            selected_reset_crash_boundary(),
        ),
    );
    let scheduler = manager
        .bootstrap(selector(fixture.main()), observation())
        .expect("child workspace must bootstrap");
    let previous_workspace_uuid = scheduler
        .context_snapshot()
        .binding()
        .identity()
        .workspace_uuid()
        .clone();
    fs::write(
        report_path,
        format!(
            "{}\n{}\n{}\n{}\n",
            fixture.temporary_path().display(),
            previous_workspace_uuid,
            fs::metadata(scheduler.context_snapshot().database_path())
                .expect("child source database must exist")
                .dev(),
            fs::metadata(scheduler.context_snapshot().database_path())
                .expect("child source database must exist")
                .ino(),
        ),
    )
    .expect("child crash recovery report must be durable before reset");
    let dispatcher = compose_dispatcher_v1(
        manager,
        WorkerIdV1::new("phase5-reset-crash-child").expect("fixture worker ID must be valid"),
    );
    let (request, slice) = reset_request(fixture.main(), &previous_workspace_uuid);
    let _ = dispatcher.dispatch(&request, &slice);
    panic!("configured reset failpoint must abort the child process");
}

#[cfg(unix)]
#[test]
fn reset_all_crash_boundaries_resume_once_without_duplicate_effects() {
    for (id, boundary, database_must_exist) in [
        ("C14", ResetAllCrashBoundaryV1::MarkerCreated, true),
        ("C15", ResetAllCrashBoundaryV1::OldDatabaseDeleted, false),
        (
            "C16",
            ResetAllCrashBoundaryV1::NewTargetDatabaseCreated,
            true,
        ),
    ] {
        let report_path = std::env::temp_dir().join(format!(
            "podway-phase5-reset-crash-{id}-{}",
            std::process::id()
        ));
        let status = Command::new(std::env::current_exe().expect("test executable must resolve"))
            .args(["--exact", RESET_CRASH_CHILD_TEST, "--nocapture"])
            .env(RESET_CRASH_REPORT_ENV, &report_path)
            .env(RESET_CRASH_BOUNDARY_ENV, format!("{boundary:?}"))
            .status()
            .expect("reset crash child must start");
        assert_eq!(
            status.signal(),
            Some(nix::libc::SIGABRT),
            "{id} child must terminate with SIGABRT at its selected boundary: {status:?}"
        );

        let report = fs::read_to_string(&report_path)
            .expect("aborted child must leave its restart recovery report");
        let mut report_lines = report.lines();
        let fixture_root = PathBuf::from(
            report_lines
                .next()
                .expect("report must contain fixture root"),
        );
        let previous_workspace_uuid = WorkspaceId::new(
            report_lines
                .next()
                .expect("report must contain predecessor workspace UUID"),
        )
        .expect("reported predecessor UUID must be valid");
        let workspace_root = fixture_root.join("main");
        let source_database_identity = (
            report_lines
                .next()
                .expect("report must contain source database device")
                .parse::<u64>()
                .expect("reported source database device must be numeric"),
            report_lines
                .next()
                .expect("report must contain source database inode")
                .parse::<u64>()
                .expect("reported source database inode must be numeric"),
        );
        assert!(
            report_lines.next().is_none(),
            "report must contain exactly the durable pre-reset state"
        );
        let runtime_path = workspace_root.join(".podway/runtime");
        let marker = ResetMarkerV1::decode_canonical(
            &fs::read(runtime_path.join("reset.marker"))
                .expect("{id} crash must retain the durable reset marker"),
        )
        .expect("{id} marker must remain canonical after SIGABRT");
        assert_eq!(
            marker.previous_workspace_uuid(),
            &previous_workspace_uuid,
            "{id} marker must preserve its durable predecessor"
        );
        let database_path = runtime_path.join("state.sqlite3");
        assert_eq!(
            database_path.exists(),
            database_must_exist,
            "{id} crash durable database state must match its exact boundary"
        );
        let crashed_database_identity = database_must_exist.then(|| {
            let identity =
                fs::metadata(&database_path).expect("{id} expected database must be inspectable");
            (identity.dev(), identity.ino())
        });
        if let Some(crashed_database_identity) = crashed_database_identity {
            if boundary == ResetAllCrashBoundaryV1::MarkerCreated {
                assert_eq!(
                    crashed_database_identity, source_database_identity,
                    "{id} must abort before deleting the predecessor database"
                );
            } else {
                assert_ne!(
                    crashed_database_identity, source_database_identity,
                    "{id} must abort after durably creating the replacement database"
                );
            }
        }
        let c16_published_database = (id == "C16").then(|| {
            let identity = crashed_database_identity
                .expect("C16 must publish its replacement database before recovery");
            let receipt_rows = terminal_receipt_rows(&database_path);
            let job_prefix = format!(
                "jobs\u{1f}{}\u{1f}1\u{1f}{}\u{1f}{}",
                marker.operation_id(),
                marker.idempotency_key().as_str(),
                marker.request_digest(),
            );
            let idempotency_prefix = format!(
                "idempotency_records\u{1f}{}\u{1f}{}\u{1f}{}",
                marker.idempotency_key().as_str(),
                marker.request_digest(),
                marker.operation_id(),
            );
            assert_eq!(
                receipt_rows.lines().count(),
                2,
                "C16 published replacement must contain exactly its terminal job and idempotency rows"
            );
            assert!(
                receipt_rows.lines().any(|row| row.starts_with(&job_prefix)),
                "C16 published replacement must retain the exact terminal job receipt identity"
            );
            assert!(
                receipt_rows
                    .lines()
                    .any(|row| row.starts_with(&idempotency_prefix)),
                "C16 published replacement must retain the exact idempotency terminal receipt identity"
            );
            (
                identity,
                fs::read(&database_path)
                    .expect("C16 published replacement database must remain readable"),
                receipt_rows,
            )
        });
        let runtime_manager = Arc::new(manager(&fixture_root));
        let dispatcher = compose_dispatcher_v1(
            Arc::clone(&runtime_manager),
            WorkerIdV1::new(format!("phase5-reset-recovery-{id}"))
                .expect("fixture worker ID must be valid"),
        );
        let (lookup_request, lookup_slice) =
            reset_lookup_request(&workspace_root, &previous_workspace_uuid, &marker);
        let lookup = dispatcher.dispatch(&lookup_request, &lookup_slice);
        let ResponseEnvelopeV1::Output(lookup) = lookup else {
            panic!("{id} marker-bound lookup must remain reconcilable: {lookup:?}");
        };
        assert_eq!(lookup.result()["found"], true, "{id}");
        assert_eq!(
            lookup.result()["job"]["id"],
            marker.operation_id().as_str(),
            "{id}"
        );
        assert_eq!(lookup.result()["job"]["sequence"], 1, "{id}");
        assert_eq!(
            lookup.result()["job"]["command"],
            "workspace.reset_all",
            "{id}"
        );
        assert_eq!(
            lookup.result()["job"]["request_digest"],
            marker.request_digest().as_str(),
            "{id}"
        );
        assert_eq!(
            lookup.result()["job"]["state"],
            if boundary == ResetAllCrashBoundaryV1::NewTargetDatabaseCreated {
                "succeeded"
            } else {
                "running"
            },
            "{id} target Store terminal receipt must win over marker-only projection"
        );
        assert_eq!(
            lookup
                .workspace()
                .expect("marker lookup must project target workspace")
                .uuid(),
            marker.target_workspace_uuid(),
            "{id}"
        );
        let (request, slice) = reset_request(&workspace_root, &previous_workspace_uuid);
        let recovered = dispatcher.dispatch(&request, &slice);
        let ResponseEnvelopeV1::Output(output) = &recovered else {
            panic!("{id} restart must recover the marker-bound reset: {recovered:?}");
        };
        let target = output
            .workspace()
            .expect("recovered reset must project its target workspace");
        let completed_job = output
            .job()
            .expect("recovered reset must project its terminal job");
        assert_eq!(completed_job.id(), marker.operation_id());
        assert_eq!(completed_job.sequence(), 1);
        assert_eq!(target.uuid(), marker.target_workspace_uuid());
        assert_eq!(target.latest_workspace_sequence(), 1);
        assert!(
            !workspace_root.join(".podway/runtime/reset.marker").exists(),
            "{id} recovery must consume the durable reset marker"
        );
        let c16_recovered_database = (id == "C16").then(|| {
            let metadata = fs::metadata(&database_path)
                .expect("C16 recovery must retain its published database");
            let identity = (metadata.dev(), metadata.ino());
            let recovered_database_bytes = fs::read(&database_path)
                .expect("C16 replacement database must remain readable after recovery");
            let recovered_receipt_rows = terminal_receipt_rows(&database_path);
            let (published_database_identity, published_database_bytes, published_receipt_rows) =
                c16_published_database
                    .as_ref()
                    .expect("C16 must snapshot its replacement before recovery");
            assert_eq!(
                identity, *published_database_identity,
                "C16 recovery must not recreate or replace the published database"
            );
            assert_eq!(
                recovered_database_bytes, *published_database_bytes,
                "C16 recovery must not rewrite the published database bytes"
            );
            assert_eq!(
                recovered_receipt_rows, *published_receipt_rows,
                "C16 recovery must not rewrite either exact terminal receipt row"
            );
            (identity, recovered_database_bytes, recovered_receipt_rows)
        });

        let replay_manager = Arc::new(manager(&fixture_root));
        let replay_dispatcher = compose_dispatcher_v1(
            Arc::clone(&replay_manager),
            WorkerIdV1::new(format!("phase5-reset-replay-{id}"))
                .expect("fixture worker ID must be valid"),
        );
        let replay = replay_dispatcher.dispatch(&request, &slice);
        let ResponseEnvelopeV1::Output(replayed_output) = &replay else {
            panic!("{id} replay must return the recovered terminal effect: {replay:?}");
        };
        assert_eq!(replayed_output.workspace(), output.workspace());
        assert_eq!(replayed_output.job(), output.job());
        assert_eq!(replayed_output.result(), output.result());
        let active = replay_manager
            .resolve_existing(
                selector(&workspace_root),
                Some(target.uuid()),
                observation(),
            )
            .expect("recovered target must remain uniquely resolvable");
        let context = active.context_snapshot();
        let stored = context
            .store()
            .read_job(context.binding().identity(), completed_job.id())
            .expect("recovered target Store must remain readable")
            .expect("recovered terminal job must remain durable");
        assert_eq!(stored.job().identity_sequence(), 1);
        if id == "C16" {
            let (recovered_database_identity, _, _) = c16_recovered_database
                .expect("C16 recovery must retain the replacement database before replay");
            let (published_database_identity, published_database_bytes, published_receipt_rows) =
                c16_published_database
                    .expect("C16 must retain its pre-recovery replacement snapshot");
            assert_eq!(
                recovered_database_identity, published_database_identity,
                "C16 recovery must not recreate or replace the published database"
            );
            assert_eq!(stored.state(), StoreJobStateV1::Succeeded);
            assert_eq!(stored.job().job_id(), marker.operation_id());
            assert_eq!(stored.job().request_digest(), marker.request_digest());
            assert_eq!(stored.submitted_at(), marker.submitted_at_ms());
            assert_eq!(
                stored.claimed_at(),
                None,
                "C16 recovery completes the marker-owned reset without worker claiming"
            );
            assert!(
                stored.finished_at().is_some(),
                "C16 terminal job must retain its finish time"
            );
            let terminal = stored
                .terminal_receipt()
                .expect("C16 terminal job must retain its durable receipt");
            assert_eq!(terminal.job(), stored.job());
            assert!(matches!(
                terminal.result(),
                podway_store::codec::PersistedTerminalResultV1::Success(
                    podway_store::codec::PersistedDomainResultV1::WorkspaceReset {
                        workspace_id,
                        revision
                    }
                ) if *workspace_id == *target.uuid() && *revision == RevisionV1::ZERO
            ));
            let projection = terminal
                .job_projection()
                .expect("C16 terminal receipt must retain its job projection");
            assert_eq!(projection.state(), PersistedTerminalJobStateV1::Succeeded);
            assert_eq!(projection.submitted_at(), stored.submitted_at());
            assert_eq!(
                projection.finished_at(),
                stored
                    .finished_at()
                    .expect("C16 terminal job must have a finish time")
            );
            assert!(
                terminal.session_projection().is_none(),
                "C16 reset receipt must not retain a session projection"
            );
            assert_eq!(
                context
                    .store()
                    .read_idempotent_outcome(
                        context.binding().identity(),
                        marker.idempotency_key(),
                        marker.request_digest(),
                    )
                    .expect("C16 idempotency record must remain readable"),
                Some(AdmitOutcomeV1::Existing(
                    JobReceiptOrTerminalV1::TerminalReceipt(terminal.clone())
                )),
                "C16 terminal receipt must retain the marker idempotency identity"
            );
            assert_eq!(
                context
                    .store()
                    .list_jobs(
                        context.binding().identity(),
                        JobListQueryV1::new(2).expect("C16 job query must be valid"),
                    )
                    .expect("C16 durable job history must remain readable"),
                vec![stored.clone()],
                "C16 recovery must have exactly one durable reset mutation"
            );
            assert_eq!(
                fs::read(&database_path)
                    .expect("C16 replayed replacement database must remain readable"),
                published_database_bytes,
                "C16 replay must be byte-equivalent and must not apply a second replacement"
            );
            assert_eq!(
                terminal_receipt_rows(&database_path),
                published_receipt_rows,
                "C16 replay must preserve both exact terminal receipt rows"
            );
            let metadata = fs::metadata(&database_path)
                .expect("C16 replayed replacement database must remain inspectable");
            assert_eq!(
                (metadata.dev(), metadata.ino()),
                crashed_database_identity
                    .expect("C16 must retain the replacement database before recovery"),
                "C16 replay must preserve the original replacement database identity"
            );
        }
        fs::remove_file(&report_path).expect("crash report must be removed");
        fs::remove_dir_all(&fixture_root).expect("crash fixture must be removed");
    }
}
