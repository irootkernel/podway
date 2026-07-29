//! Real-Git/SQLite coverage for the G005 production composition.

use crate::support_phase4_workspace;

use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Barrier},
    thread,
};

#[cfg(unix)]
use std::os::unix::{ffi::OsStrExt, fs::PermissionsExt};

use podway_core::{DomainError, JobId, Revision, Sha256Digest, UnixMillis, canonicalize_json_v1};
use podway_daemon::{
    dispatch::WorkspaceRuntimeV1,
    production::{ProductionWorkspaceRuntimeV1, compose_dispatcher_v1},
    runtime_workspace::WorkspaceRuntimeObservationV1,
    server::RequestDispatcherV1,
};
use podway_git::{GitResolverContractV1, NativeGitResolverV1};
use podway_protocol::{
    ClientInfoV1, CommandNameV1, IdempotencyKeyV1, JobStateV1, NextResultV1, OperationV1,
    OutputEnvelopeV1, PreconditionsV1, RequestEnvelopeInputV1, RequestEnvelopeV1, RequestIdV1,
    RequestOptionsV1, ResponseEnvelopeV1, Rfc3339MillisV1, SliceRequestV1, StageStatusResultV1,
    StatusResultV1, WorkspaceContextV1, WorktreeSelectorWireV1,
};
use podway_service::ServiceRuntimePathsV1;
use podway_store::{
    AdmissionSessionIdentityV1, AdmitOutcomeV1, AdmitRequestV1, CommandV1,
    IdempotencyKeyV1 as StoreIdempotencyKeyV1, JobListQueryV1, PersistedResponseContextV1,
    RevisionAttemptItemPreconditionsV1, SqliteStoreOptionsV1, SqliteStoreV1, StoreContractV1,
    StoreReadContractV1, TerminalResultV1, WorkerIdV1,
};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
#[cfg(unix)]
use support_phase4_workspace::non_utf8_child_path;
use support_phase4_workspace::{copy_tree, git_worktrees, read_file, selector as git_selector};

fn fixture_runtime_directory(root: &Path) -> PathBuf {
    let root = fs::canonicalize(root).expect("fixture root must canonicalize");
    #[cfg(unix)]
    let digest = Sha256::digest(root.as_os_str().as_bytes());
    #[cfg(not(unix))]
    let digest = Sha256::digest(root.to_string_lossy().as_bytes());
    let digest = format!("{digest:x}");
    std::env::temp_dir().join(format!("pdr-{}", &digest[..16]))
}

fn manager(root: &Path) -> podway_daemon::runtime_workspace::WorkspaceRuntimeManagerV1 {
    let application_support = root.join("Application Support");
    fs::create_dir_all(&application_support).unwrap();
    #[cfg(unix)]
    fs::set_permissions(&application_support, fs::Permissions::from_mode(0o700)).unwrap();
    let paths = ServiceRuntimePathsV1::from_directories(
        root.join("LaunchAgents"),
        application_support.join("Podway"),
        root.join("Logs/Podway"),
        fixture_runtime_directory(root),
    )
    .unwrap();
    podway_daemon::runtime_workspace::WorkspaceRuntimeManagerV1::new(
        &paths,
        SqliteStoreOptionsV1::new(8).unwrap(),
    )
}

fn make_runtime_private(root: &Path) {
    #[cfg(unix)]
    fs::set_permissions(
        root.join(".podway/runtime"),
        fs::Permissions::from_mode(0o700),
    )
    .unwrap();
}

fn selector(path: &Path) -> WorktreeSelectorWireV1 {
    let canonical = fs::canonicalize(path).unwrap();
    #[cfg(unix)]
    let bytes = canonical.as_os_str().as_bytes();
    #[cfg(not(unix))]
    let bytes = canonical.to_string_lossy().as_bytes();
    WorktreeSelectorWireV1::new(bytes, canonical.display().to_string(), None).unwrap()
}
fn observation() -> WorkspaceRuntimeObservationV1 {
    WorkspaceRuntimeObservationV1::new(
        UnixMillis::new(1_700_000_000_123),
        Rfc3339MillisV1::new("2026-07-15T12:34:56.789Z")
            .expect("fixture observation timestamp must be valid"),
    )
}

fn request(
    request_number: u64,
    command: &str,
    selector: &WorktreeSelectorWireV1,
    payload: Value,
    options: RequestOptionsV1,
    idempotency_key: &str,
    preconditions: PreconditionsV1,
) -> (RequestEnvelopeV1, SliceRequestV1) {
    let operation = match command {
        "workspace.init" => OperationV1::Bootstrap,
        "workspace.repair" | "job.cancel" => OperationV1::Control,
        "workspace.doctor" | "session.status" | "session.next" | "job.list" | "job.lookup"
        | "job.status" | "job.wait" => OperationV1::Query,
        _ => OperationV1::Mutate,
    };
    let envelope = RequestEnvelopeV1::new(RequestEnvelopeInputV1 {
        request_id: RequestIdV1::new(format!("00000000-0000-4000-8000-{request_number:012x}"))
            .unwrap(),
        client: ClientInfoV1::new("production-test", "1", 1).unwrap(),
        operation,
        command: CommandNameV1::new(command).unwrap(),
        workspace: Some(WorkspaceContextV1::new(selector.display(), None).unwrap()),
        idempotency_key: matches!(operation, OperationV1::Bootstrap | OperationV1::Mutate)
            .then(|| IdempotencyKeyV1::new(idempotency_key).unwrap()),
        preconditions,
        options,
        payload: payload.as_object().unwrap().clone(),
    })
    .unwrap();
    let slice = SliceRequestV1::from_envelope(&envelope).unwrap();
    (envelope, slice)
}

fn dispatch_command(
    dispatcher: &impl RequestDispatcherV1,
    request: &(RequestEnvelopeV1, SliceRequestV1),
    command: &str,
) -> OutputEnvelopeV1 {
    match dispatcher.dispatch(&request.0, &request.1) {
        ResponseEnvelopeV1::Output(output) => {
            assert_eq!(output.command().as_str(), command);
            output
        }
        ResponseEnvelopeV1::Error(error) => panic!(
            "expected successful {command} output, got {}: {}",
            error.code().as_str(),
            error.message()
        ),
    }
}

fn status_result(output: &OutputEnvelopeV1) -> StatusResultV1 {
    StatusResultV1::from_result_map(output.result())
        .expect("session.status must return the documented status result")
}

fn next_result(output: &OutputEnvelopeV1) -> NextResultV1 {
    NextResultV1::from_result_map(output.result())
        .expect("session.next must return the documented next result")
}

fn session_preconditions(status: &StatusResultV1) -> PreconditionsV1 {
    let current = status
        .current
        .as_ref()
        .expect("running session must have a current attempt");
    PreconditionsV1::new(
        Some(status.session.id.clone()),
        Some(status.session.revision),
        Some(current.attempt_id.clone()),
        None,
        None,
        None,
    )
    .unwrap()
}

fn item_preconditions(status: &StatusResultV1, item_id: &str) -> PreconditionsV1 {
    let current = status
        .current
        .as_ref()
        .expect("running session must have a current attempt");
    let item = status
        .items
        .iter()
        .find(|item| item.id.as_str() == item_id)
        .expect("current status must include the requested item");
    PreconditionsV1::new(
        Some(status.session.id.clone()),
        None,
        Some(current.attempt_id.clone()),
        Some(item.revision),
        None,
        None,
    )
    .unwrap()
}

#[test]
fn production_composition_bootstraps_replays_and_covers_active_attempt_retry_return() {
    let fixture = git_worktrees();
    make_runtime_private(fixture.main());
    let manager = Arc::new(manager(fixture.temporary_path()));
    let dispatcher = compose_dispatcher_v1(
        Arc::clone(&manager),
        WorkerIdV1::new("production-composition-test").unwrap(),
    );
    let main_selector = selector(fixture.main());

    let initialize = request(
        1,
        "workspace.init",
        &main_selector,
        json!({"selector": serde_json::to_value(&main_selector).unwrap()}),
        RequestOptionsV1::new(false, 5_000).unwrap(),
        "initialize-main",
        PreconditionsV1::default(),
    );
    let initialized = dispatch_command(&dispatcher, &initialize, "workspace.init");
    assert!(
        initialized.session().is_none(),
        "workspace.init must succeed before a session exists"
    );
    assert!(
        initialized
            .job()
            .expect("workspace.init must produce a durable job")
            .finished_at()
            .is_some(),
        "workspace.init must return a terminal job"
    );

    let preset = request(
        2,
        "session.start",
        &main_selector,
        json!({
            "selector": serde_json::to_value(&main_selector).unwrap(),
            "preset": "sw-dev",
            "task_title": "Production composition task"
        }),
        RequestOptionsV1::new(false, 5_000).unwrap(),
        "preset-start",
        PreconditionsV1::default(),
    );
    let started = dispatch_command(&dispatcher, &preset, "session.start");
    assert!(
        started.session().is_some(),
        "session.start terminal output must project the persisted session"
    );
    let lookup = request(
        2_000,
        "job.lookup",
        &main_selector,
        json!({
            "selector": serde_json::to_value(&main_selector).unwrap(),
            "idempotency_key": "preset-start"
        }),
        RequestOptionsV1::new(false, 0).unwrap(),
        "unused-lookup-envelope-key",
        PreconditionsV1::default(),
    );
    assert!(lookup.0.idempotency_key().is_none());
    let lookup = dispatch_command(&dispatcher, &lookup, "job.lookup");
    assert_eq!(lookup.result()["found"], true);
    assert_eq!(
        lookup.result()["job"]["id"],
        started
            .job()
            .expect("start must expose its job")
            .id()
            .as_str()
    );
    assert_eq!(lookup.result()["job"]["command"], "session.start");
    assert_eq!(lookup.result()["job"]["state"], "succeeded");
    assert!(
        lookup.result()["job"]["request_digest"]
            .as_str()
            .is_some_and(|digest| digest.starts_with("sha256:")),
        "lookup must expose the canonical request digest"
    );
    let missing_lookup = request(
        2_002,
        "job.lookup",
        &main_selector,
        json!({
            "selector": serde_json::to_value(&main_selector).unwrap(),
            "idempotency_key": "missing-key"
        }),
        RequestOptionsV1::new(false, 0).unwrap(),
        "unused-missing-lookup-key",
        PreconditionsV1::default(),
    );
    let missing_lookup = dispatch_command(&dispatcher, &missing_lookup, "job.lookup");
    assert_eq!(missing_lookup.result()["found"], false);
    assert_eq!(missing_lookup.result().len(), 1);
    let runtime = manager
        .resolve_existing(git_selector(fixture.main()), None, observation())
        .expect("started workspace must resolve through the manager");
    let context = runtime.context_snapshot();
    let identity = context.binding().identity();
    let session_before_duplicate = context
        .store()
        .read_session_aggregate(identity)
        .expect("started session must be readable")
        .expect("started session must exist");
    let sequence_before_duplicate = context
        .store()
        .read_workspace_view(identity)
        .expect("started workspace view must be readable")
        .latest_workspace_sequence();
    let jobs_before_duplicate = context
        .store()
        .list_jobs(
            identity,
            JobListQueryV1::new(100).expect("duplicate start job query must be valid"),
        )
        .expect("started jobs must be readable");
    let duplicate_start = request(
        2_001,
        "session.start",
        &main_selector,
        json!({
            "selector": serde_json::to_value(&main_selector).unwrap(),
            "preset": "sw-dev",
            "task_title": "Must not replace the current task"
        }),
        RequestOptionsV1::new(false, 5_000).unwrap(),
        "duplicate-preset-start",
        PreconditionsV1::default(),
    );
    let ResponseEnvelopeV1::Error(duplicate_error) =
        dispatcher.dispatch(&duplicate_start.0, &duplicate_start.1)
    else {
        panic!("a second plain start must fail before durable admission");
    };
    assert_eq!(duplicate_error.code().as_str(), "SESSION_ALREADY_EXISTS");
    assert_eq!(duplicate_error.exit_code().get(), 1);
    assert!(!duplicate_error.retryable());
    assert_eq!(
        context
            .store()
            .read_session_aggregate(identity)
            .expect("session must remain readable")
            .expect("session must remain present"),
        session_before_duplicate
    );
    assert_eq!(
        context
            .store()
            .list_jobs(
                identity,
                JobListQueryV1::new(100).expect("duplicate start job query must remain valid"),
            )
            .expect("jobs must remain readable"),
        jobs_before_duplicate
    );
    assert_eq!(
        context
            .store()
            .read_workspace_view(identity)
            .expect("workspace view must remain readable")
            .latest_workspace_sequence(),
        sequence_before_duplicate
    );

    let status = request(
        3,
        "session.status",
        &main_selector,
        json!({"selector": serde_json::to_value(&main_selector).unwrap()}),
        RequestOptionsV1::new(false, 0).unwrap(),
        "unused-status-key",
        PreconditionsV1::default(),
    );
    let initial_status = status_result(&dispatch_command(&dispatcher, &status, "session.status"));
    assert_eq!(
        started.result()["procedure_digest"],
        initial_status.task.procedure.digest.as_str(),
        "session.start must return the exact digest observed by later status reads"
    );
    let started_session = started
        .session()
        .expect("session.start terminal output includes a session");
    assert_eq!(started_session.id(), &initial_status.session.id);
    assert_eq!(
        started_session.revision_after(),
        initial_status.session.revision,
        "initial terminal projection uses the persisted post-mutation revision"
    );
    let initial_current = initial_status
        .current
        .as_ref()
        .expect("session.start must create a current attempt");
    assert_eq!(initial_status.task.title, "Production composition task");
    assert_eq!(initial_status.task.procedure.id, "sw-dev");
    assert_eq!(initial_current.stage_id.as_str(), "understand");
    assert_eq!(
        initial_current.attempt_number, 1,
        "[FIRST-CORRECTNESS-ACTIVE-ATTEMPT] initial attempt must be attempt one"
    );
    assert_eq!(
        initial_status
            .stages
            .iter()
            .filter(|stage| stage.status == StageStatusResultV1::Current)
            .count(),
        1,
        "[FIRST-CORRECTNESS-ACTIVE-ATTEMPT] exactly one stage must expose the active attempt"
    );
    assert_eq!(
        initial_status
            .stages
            .iter()
            .find(|stage| stage.id.as_str() == "understand")
            .expect("sw-dev must include the understand stage")
            .latest_attempt_number,
        1,
        "[FIRST-CORRECTNESS-ACTIVE-ATTEMPT] replay must not create another initial attempt"
    );

    let next = request(
        4,
        "session.next",
        &main_selector,
        json!({"selector": serde_json::to_value(&main_selector).unwrap()}),
        RequestOptionsV1::new(false, 0).unwrap(),
        "unused-next-key",
        PreconditionsV1::default(),
    );
    let initial_next = next_result(&dispatch_command(&dispatcher, &next, "session.next"));
    let next_stage = initial_next
        .stage
        .as_ref()
        .expect("session.next must identify the active stage");
    assert_eq!(next_stage.id.as_str(), "understand");
    assert_eq!(next_stage.attempt_id, initial_current.attempt_id);
    assert!(
        initial_next
            .missing_required_items
            .iter()
            .any(|item| item.id.as_str() == "goal"),
        "session.next must identify the missing goal item"
    );
    assert!(
        initial_next
            .suggestions
            .iter()
            .any(|suggestion| suggestion.command == "item.set"),
        "session.next must suggest a concrete item mutation"
    );

    let seed_goal = request(
        5,
        "item.set",
        &main_selector,
        json!({
            "selector": serde_json::to_value(&main_selector).unwrap(),
            "item_id": "goal",
            "value": "discard this value on clean retry"
        }),
        RequestOptionsV1::new(false, 5_000).unwrap(),
        "seed-goal-before-retry",
        item_preconditions(&initial_status, "goal"),
    );
    let seeded_goal_output = dispatch_command(&dispatcher, &seed_goal, "item.set");

    let status = request(
        6,
        "session.status",
        &main_selector,
        json!({"selector": serde_json::to_value(&main_selector).unwrap()}),
        RequestOptionsV1::new(false, 0).unwrap(),
        "unused-seeded-status-key",
        PreconditionsV1::default(),
    );
    let seeded_status = status_result(&dispatch_command(&dispatcher, &status, "session.status"));
    let seeded_goal_session = seeded_goal_output
        .session()
        .expect("item mutation terminal output includes a session");
    assert_eq!(seeded_goal_session.id(), &seeded_status.session.id);
    assert_eq!(
        seeded_goal_session.revision_after(),
        seeded_status.session.revision
    );
    assert_eq!(
        seeded_status
            .items
            .iter()
            .find(|item| item.id.as_str() == "goal")
            .expect("status must include the goal item")
            .value,
        json!("discard this value on clean retry")
    );

    let retry = request(
        7,
        "session.retry",
        &main_selector,
        json!({
            "selector": serde_json::to_value(&main_selector).unwrap(),
            "reason": "restart the active attempt cleanly"
        }),
        RequestOptionsV1::new(false, 5_000).unwrap(),
        "clean-retry",
        session_preconditions(&seeded_status),
    );
    let retry_output = dispatch_command(&dispatcher, &retry, "session.retry");

    let status = request(
        8,
        "session.status",
        &main_selector,
        json!({"selector": serde_json::to_value(&main_selector).unwrap()}),
        RequestOptionsV1::new(false, 0).unwrap(),
        "unused-retry-status-key",
        PreconditionsV1::default(),
    );
    let retried_status = status_result(&dispatch_command(&dispatcher, &status, "session.status"));
    let retry_session = retry_output
        .session()
        .expect("retry terminal output includes a session");
    assert_eq!(retry_session.id(), &retried_status.session.id);
    assert_eq!(
        retry_session.revision_after(),
        retried_status.session.revision
    );
    let retried_current = retried_status
        .current
        .as_ref()
        .expect("clean retry must create a current attempt");
    assert_ne!(
        retried_current.attempt_id, initial_current.attempt_id,
        "[FIRST-CORRECTNESS-RETRY] retry must create a fresh attempt"
    );
    assert_eq!(
        retried_current.attempt_number, 2,
        "[FIRST-CORRECTNESS-RETRY] retry must advance the current stage attempt number"
    );
    assert_eq!(
        retried_status
            .items
            .iter()
            .find(|item| item.id.as_str() == "goal")
            .expect("retried status must include the goal item")
            .value,
        Value::Null,
        "[FIRST-CORRECTNESS-RETRY] fresh retry must not copy prior item values"
    );

    let set_goal = request(
        9,
        "item.set",
        &main_selector,
        json!({
            "selector": serde_json::to_value(&main_selector).unwrap(),
            "item_id": "goal",
            "value": "complete the production integration path"
        }),
        RequestOptionsV1::new(false, 5_000).unwrap(),
        "set-goal-after-retry",
        item_preconditions(&retried_status, "goal"),
    );
    dispatch_command(&dispatcher, &set_goal, "item.set");

    let status = request(
        10,
        "session.status",
        &main_selector,
        json!({"selector": serde_json::to_value(&main_selector).unwrap()}),
        RequestOptionsV1::new(false, 0).unwrap(),
        "unused-goal-status-key",
        PreconditionsV1::default(),
    );
    let goal_status = status_result(&dispatch_command(&dispatcher, &status, "session.status"));
    let add_acceptance = request(
        11,
        "item.add",
        &main_selector,
        json!({
            "selector": serde_json::to_value(&main_selector).unwrap(),
            "item_id": "acceptance-criteria",
            "value": "exercise the production dispatcher"
        }),
        RequestOptionsV1::new(false, 5_000).unwrap(),
        "add-acceptance-after-retry",
        item_preconditions(&goal_status, "acceptance-criteria"),
    );
    dispatch_command(&dispatcher, &add_acceptance, "item.add");

    let status = request(
        12,
        "session.status",
        &main_selector,
        json!({"selector": serde_json::to_value(&main_selector).unwrap()}),
        RequestOptionsV1::new(false, 0).unwrap(),
        "unused-ready-status-key",
        PreconditionsV1::default(),
    );
    let ready_status = status_result(&dispatch_command(&dispatcher, &status, "session.status"));
    assert!(
        ready_status
            .current
            .as_ref()
            .expect("satisfied stage must remain current before completion")
            .ready_to_complete,
        "required item mutations must make the active stage completable"
    );

    let complete = request(
        13,
        "session.complete",
        &main_selector,
        json!({"selector": serde_json::to_value(&main_selector).unwrap()}),
        RequestOptionsV1::new(false, 5_000).unwrap(),
        "complete-understand",
        session_preconditions(&ready_status),
    );
    dispatch_command(&dispatcher, &complete, "session.complete");

    let status = request(
        14,
        "session.status",
        &main_selector,
        json!({"selector": serde_json::to_value(&main_selector).unwrap()}),
        RequestOptionsV1::new(false, 0).unwrap(),
        "unused-inspect-status-key",
        PreconditionsV1::default(),
    );
    let inspect_status = status_result(&dispatch_command(&dispatcher, &status, "session.status"));
    assert_eq!(
        inspect_status
            .current
            .as_ref()
            .expect("completion must enter the next stage")
            .stage_id
            .as_str(),
        "inspect"
    );

    let return_to_understand = request(
        15,
        "session.return",
        &main_selector,
        json!({
            "selector": serde_json::to_value(&main_selector).unwrap(),
            "destination_stage_id": "understand",
            "reason": "revisit the completed stage"
        }),
        RequestOptionsV1::new(false, 5_000).unwrap(),
        "return-to-understand",
        session_preconditions(&inspect_status),
    );
    let return_output = dispatch_command(&dispatcher, &return_to_understand, "session.return");

    let status = request(
        16,
        "session.status",
        &main_selector,
        json!({"selector": serde_json::to_value(&main_selector).unwrap()}),
        RequestOptionsV1::new(false, 0).unwrap(),
        "unused-return-status-key",
        PreconditionsV1::default(),
    );
    let returned_status = status_result(&dispatch_command(&dispatcher, &status, "session.status"));
    let return_session = return_output
        .session()
        .expect("return terminal output includes a session");
    assert_eq!(return_session.id(), &returned_status.session.id);
    assert_eq!(
        return_session.revision_after(),
        returned_status.session.revision
    );
    let returned_current = returned_status
        .current
        .as_ref()
        .expect("return must create a current destination attempt");
    assert_eq!(
        returned_current.stage_id.as_str(),
        "understand",
        "[FIRST-CORRECTNESS-RETURN] return must activate the requested destination stage"
    );
    assert_eq!(
        returned_current.attempt_number, 3,
        "[FIRST-CORRECTNESS-RETURN] return must start a fresh destination attempt"
    );
    assert_eq!(
        returned_status
            .stages
            .iter()
            .find(|stage| stage.id.as_str() == "inspect")
            .expect("sw-dev must include the inspect stage")
            .status,
        StageStatusResultV1::Redo,
        "[FIRST-CORRECTNESS-RETURN] return must mark the abandoned source stage for redo"
    );
    assert_eq!(
        returned_status
            .items
            .iter()
            .find(|item| item.id.as_str() == "goal")
            .expect("returned status must include destination items")
            .value,
        Value::Null,
        "[FIRST-CORRECTNESS-RETURN] return must start the destination with clean item values"
    );
    let replay = request(
        17,
        "session.start",
        &main_selector,
        json!({
            "selector": serde_json::to_value(&main_selector).unwrap(),
            "preset": "sw-dev",
            "task_title": "Production composition task"
        }),
        RequestOptionsV1::new(false, 5_000).unwrap(),
        "preset-start",
        PreconditionsV1::default(),
    );
    let reopened_dispatcher = compose_dispatcher_v1(
        Arc::new(self::manager(fixture.temporary_path())),
        WorkerIdV1::new("production-replay-after-reopen-test").unwrap(),
    );
    let replayed = dispatch_command(&reopened_dispatcher, &replay, "session.start");
    assert_ne!(
        started_session.revision_after(),
        returned_status.session.revision,
        "later session mutations must change the live session before replay"
    );
    assert_eq!(
        replayed.request_id(),
        replay.0.request_id(),
        "terminal replay must correlate to the retry request"
    );
    assert_ne!(
        replayed.request_id(),
        started.request_id(),
        "terminal replay must not reuse the original response correlation"
    );
    assert_eq!(
        started.job(),
        replayed.job(),
        "reopened replay must preserve the immutable terminal job projection"
    );
    assert_eq!(
        started.session(),
        replayed.session(),
        "reopened replay after later mutations must preserve the immutable terminal session projection"
    );
    assert_eq!(
        started.result(),
        replayed.result(),
        "reopened replay after later mutations must preserve the immutable terminal result payload"
    );
    assert_eq!(
        started.warnings(),
        replayed.warnings(),
        "reopened replay after later mutations must preserve immutable terminal warnings"
    );
}

#[test]
fn manager_adapter_reuses_moves_rejects_copies_and_distinct_identities_admit_real_mutations() {
    let fixture = git_worktrees();
    let direct = NativeGitResolverV1::new()
        .resolve(git_selector(fixture.main()))
        .unwrap();
    assert_eq!(
        direct.roots().worktree_root().decode_path_bytes().unwrap(),
        fs::canonicalize(fixture.main())
            .unwrap()
            .as_os_str()
            .as_bytes()
    );
    make_runtime_private(fixture.main());
    make_runtime_private(fixture.linked());
    let manager = Arc::new(manager(fixture.temporary_path()));
    let clock = Arc::new(podway_daemon::production::NativeProductionClockV1::default());
    let runtime = ProductionWorkspaceRuntimeV1::new(Arc::clone(&manager), clock);
    let main_selector = selector(fixture.main());
    let main = runtime.resolve_bootstrap(&main_selector).unwrap();
    assert!(read_file(&fixture.main().join(".podway/config.yaml")).starts_with(b"schema:"));
    let linked_selector = selector(fixture.linked());
    let linked = runtime.resolve_bootstrap(&linked_selector).unwrap();
    assert!(!Arc::ptr_eq(main.scheduler(), linked.scheduler()));

    let dispatcher = Arc::new(compose_dispatcher_v1(
        Arc::clone(&manager),
        WorkerIdV1::new("production-overlap-test").unwrap(),
    ));
    let main_initialize = request(
        100,
        "session.start",
        &main_selector,
        json!({
            "selector": serde_json::to_value(&main_selector).unwrap(),
            "preset": "sw-dev",
            "task_title": "Independent main mutation"
        }),
        RequestOptionsV1::new(false, 5_000).unwrap(),
        "start-main-overlap",
        PreconditionsV1::default(),
    );
    let linked_initialize = request(
        101,
        "session.start",
        &linked_selector,
        json!({
            "selector": serde_json::to_value(&linked_selector).unwrap(),
            "preset": "sw-dev",
            "task_title": "Independent linked mutation"
        }),
        RequestOptionsV1::new(false, 5_000).unwrap(),
        "start-linked-overlap",
        PreconditionsV1::default(),
    );
    let start = Arc::new(Barrier::new(3));
    let main_mutation = {
        let dispatcher = Arc::clone(&dispatcher);
        let start = Arc::clone(&start);
        thread::spawn(move || {
            start.wait();
            dispatch_command(dispatcher.as_ref(), &main_initialize, "session.start")
        })
    };
    let linked_mutation = {
        let dispatcher = Arc::clone(&dispatcher);
        let start = Arc::clone(&start);
        thread::spawn(move || {
            start.wait();
            dispatch_command(dispatcher.as_ref(), &linked_initialize, "session.start")
        })
    };
    start.wait();
    let main_output = main_mutation
        .join()
        .expect("main identity mutation does not panic");
    let linked_output = linked_mutation
        .join()
        .expect("linked identity mutation does not panic");
    for output in [&main_output, &linked_output] {
        assert!(
            output.session().is_some(),
            "independent preset mutation must project its durable session"
        );
        assert!(
            output
                .job()
                .expect("real workspace mutation has a durable job")
                .finished_at()
                .is_some(),
            "independent identity mutation reaches its own terminal Store receipt"
        );
    }

    let workspace_id = main
        .scheduler()
        .context_snapshot()
        .binding()
        .identity()
        .workspace_uuid()
        .clone();
    #[cfg(unix)]
    let relocated = {
        let non_utf8 = non_utf8_child_path(fixture.temporary_path());
        match fs::rename(fixture.main(), &non_utf8) {
            Ok(()) => non_utf8,
            Err(error) if error.raw_os_error() == Some(92) => {
                eprintln!(
                    "SKIP non-UTF8 relocation evidence: filesystem rejected non-UTF8 path bytes: {error}"
                );
                return;
            }
            Err(error) => panic!("move worktree: {error}"),
        }
    };
    #[cfg(not(unix))]
    let relocated = {
        eprintln!("SKIP non-UTF8 relocation evidence: unsupported platform");
        return;
    };
    let moved = runtime.resolve_existing(&selector(&relocated)).unwrap();
    assert!(Arc::ptr_eq(main.scheduler(), moved.scheduler()));
    assert_eq!(
        workspace_id,
        moved
            .scheduler()
            .context_snapshot()
            .binding()
            .identity()
            .workspace_uuid()
            .clone()
    );

    let copied = fixture.temporary_path().join("copied-live-worktree");
    copy_tree(&relocated, &copied);
    assert!(runtime.resolve_existing(&selector(&copied)).is_err());
}
#[test]
fn workspace_repair_reports_a_real_move_once_and_a_replay_as_unchanged() {
    let fixture = git_worktrees();
    make_runtime_private(fixture.main());
    let manager = Arc::new(manager(fixture.temporary_path()));
    let dispatcher = compose_dispatcher_v1(
        Arc::clone(&manager),
        WorkerIdV1::new("workspace-repair-production-test").unwrap(),
    );
    let initial_selector = selector(fixture.main());
    let initialize = request(
        900,
        "workspace.init",
        &initial_selector,
        json!({"selector": serde_json::to_value(&initial_selector).unwrap()}),
        RequestOptionsV1::new(false, 5_000).unwrap(),
        "workspace-repair-initialize",
        PreconditionsV1::default(),
    );
    dispatch_command(&dispatcher, &initialize, "workspace.init");

    let relocated = fixture.temporary_path().join("relocated-main");
    fs::rename(fixture.main(), &relocated).expect("move real initialized worktree");
    let moved_selector = selector(&relocated);
    let repair = request(
        901,
        "workspace.repair",
        &moved_selector,
        json!({"selector": serde_json::to_value(&moved_selector).unwrap()}),
        RequestOptionsV1::new(false, 0).unwrap(),
        "unused-repair-key",
        PreconditionsV1::default(),
    );
    let repaired = dispatch_command(&dispatcher, &repair, "workspace.repair");
    assert_eq!(repaired.result().get("changed"), Some(&Value::Bool(true)));
    assert_eq!(
        repaired.result().get("changes"),
        Some(&Value::Array(vec![
            Value::String("workspace_binding.last_validated_root".to_owned()),
            Value::String("registry.last_known_root".to_owned()),
        ]))
    );

    let replay = request(
        902,
        "workspace.repair",
        &moved_selector,
        json!({"selector": serde_json::to_value(&moved_selector).unwrap()}),
        RequestOptionsV1::new(false, 0).unwrap(),
        "unused-repair-replay-key",
        PreconditionsV1::default(),
    );
    let replayed = dispatch_command(&dispatcher, &replay, "workspace.repair");
    assert_eq!(replayed.result().get("changed"), Some(&Value::Bool(false)));

    let copied = fixture.temporary_path().join("copied-repaired-worktree");
    copy_tree(&relocated, &copied);
    let copied_selector = selector(&copied);
    let copied_repair = request(
        903,
        "workspace.repair",
        &copied_selector,
        json!({"selector": serde_json::to_value(&copied_selector).unwrap()}),
        RequestOptionsV1::new(false, 0).unwrap(),
        "unused-copied-repair-key",
        PreconditionsV1::default(),
    );
    assert!(matches!(
        dispatcher.dispatch(&copied_repair.0, &copied_repair.1),
        ResponseEnvelopeV1::Error(_)
    ));
}
#[test]
fn workspace_repair_reconciles_a_registry_only_stale_root_without_false_failure() {
    let fixture = git_worktrees();
    make_runtime_private(fixture.main());
    let manager = Arc::new(manager(fixture.temporary_path()));
    let dispatcher = compose_dispatcher_v1(
        Arc::clone(&manager),
        WorkerIdV1::new("workspace-repair-registry-only-production-test").unwrap(),
    );
    let main_selector = selector(fixture.main());
    let linked_selector = selector(fixture.linked());
    let initialize_main = request(
        904,
        "workspace.init",
        &main_selector,
        json!({"selector": serde_json::to_value(&main_selector).unwrap()}),
        RequestOptionsV1::new(false, 5_000).unwrap(),
        "initialize-main",
        PreconditionsV1::default(),
    );
    let main_workspace_uuid = dispatch_command(&dispatcher, &initialize_main, "workspace.init")
        .workspace()
        .expect("workspace init must expose its workspace")
        .uuid()
        .as_str()
        .to_owned();
    let initialize_linked = request(
        905,
        "workspace.init",
        &linked_selector,
        json!({"selector": serde_json::to_value(&linked_selector).unwrap()}),
        RequestOptionsV1::new(false, 5_000).unwrap(),
        "initialize-linked",
        PreconditionsV1::default(),
    );
    dispatch_command(&dispatcher, &initialize_linked, "workspace.init");

    let registry_path = manager.registry().registry_path().to_path_buf();
    let mut registry: Value =
        serde_json::from_slice(&fs::read(&registry_path).unwrap()).expect("registry JSON");
    let entries = registry["workspaces"]
        .as_array_mut()
        .expect("registry workspace entries");
    let linked_root = entries
        .iter()
        .find(|entry| entry["workspace_uuid"] != main_workspace_uuid)
        .and_then(|entry| entry["last_known_root"].as_str())
        .expect("linked registry root")
        .to_owned();
    entries
        .iter_mut()
        .find(|entry| entry["workspace_uuid"] == main_workspace_uuid)
        .expect("main registry entry")["last_known_root"] = Value::String(linked_root);
    fs::write(
        &registry_path,
        canonicalize_json_v1(&registry).expect("canonical stale registry"),
    )
    .expect("write stale registry");

    let repair = request(
        907,
        "workspace.repair",
        &main_selector,
        json!({"selector": serde_json::to_value(&main_selector).unwrap()}),
        RequestOptionsV1::new(false, 0).unwrap(),
        "unused-registry-only-repair-key",
        PreconditionsV1::default(),
    );
    let repaired = dispatch_command(&dispatcher, &repair, "workspace.repair");
    assert_eq!(repaired.result().get("changed"), Some(&Value::Bool(true)));
    assert_eq!(
        repaired.result().get("changes"),
        Some(&Value::Array(vec![Value::String(
            "registry.last_known_root".to_owned()
        )]))
    );

    let replay = request(
        908,
        "workspace.repair",
        &main_selector,
        json!({"selector": serde_json::to_value(&main_selector).unwrap()}),
        RequestOptionsV1::new(false, 0).unwrap(),
        "unused-registry-only-repair-replay-key",
        PreconditionsV1::default(),
    );
    let replayed = dispatch_command(&dispatcher, &replay, "workspace.repair");
    assert_eq!(replayed.result().get("changed"), Some(&Value::Bool(false)));
}

#[test]
fn job_cancel_public_route_projects_committed_cancellation() {
    let fixture = git_worktrees();
    make_runtime_private(fixture.main());
    let runtime_manager = Arc::new(manager(fixture.temporary_path()));
    let dispatcher = compose_dispatcher_v1(
        Arc::clone(&runtime_manager),
        WorkerIdV1::new("job-cancel-production-test").unwrap(),
    );
    let workspace_selector = selector(fixture.main());
    let initialize = request(
        909,
        "workspace.init",
        &workspace_selector,
        json!({"selector": serde_json::to_value(&workspace_selector).unwrap()}),
        RequestOptionsV1::new(false, 5_000).unwrap(),
        "job-cancel-initialize",
        PreconditionsV1::default(),
    );
    dispatch_command(&dispatcher, &initialize, "workspace.init");

    let runtime = runtime_manager
        .resolve_existing(git_selector(fixture.main()), None, observation())
        .expect("initialized workspace must resolve through the manager");
    let context = runtime.context_snapshot();
    let job_id = JobId::new("00000000-0000-4000-8000-000000000911").unwrap();
    let direct_store = SqliteStoreV1::open(
        context.database_path(),
        context.workspace_root(),
        context.binding().identity().clone(),
        context.store_options().clone(),
        UnixMillis::new(1),
    )
    .expect("manager binding must reopen its Store for deterministic queued-job setup");
    let seeded = direct_store
        .admit(
            context.binding().identity(),
            AdmitRequestV1::new(
                CommandV1::WorkspaceInitialize,
                StoreIdempotencyKeyV1::new("job-cancel-seeded").unwrap(),
                job_id.clone(),
                RevisionAttemptItemPreconditionsV1::new(None, None, None, None).unwrap(),
                Sha256Digest::new(format!("sha256:{}", "9".repeat(64))).unwrap(),
                UnixMillis::new(1),
            ),
        )
        .expect("manager-owned Store must accept the queued test job");
    assert!(matches!(seeded, AdmitOutcomeV1::New(_)));
    assert_eq!(
        direct_store
            .read_job(context.binding().identity(), &job_id)
            .expect("seeded job must be readable")
            .expect("seeded job must exist")
            .state(),
        podway_store::JobStateV1::Queued
    );
    drop(direct_store);

    let cancel = request(
        911,
        "job.cancel",
        &workspace_selector,
        json!({
            "selector": serde_json::to_value(&workspace_selector).unwrap(),
            "job_id": job_id,
        }),
        RequestOptionsV1::new(false, 0).unwrap(),
        "unused-cancel-key",
        PreconditionsV1::new(None, None, None, None, None, Some(JobStateV1::Queued)).unwrap(),
    );
    let cancelled = dispatch_command(&dispatcher, &cancel, "job.cancel");
    assert_eq!(
        cancelled.result().get("cancelled"),
        Some(&Value::Bool(true)),
        "a committed cancellation must project success"
    );
    assert_eq!(
        cancelled
            .job()
            .expect("cancel response must include job")
            .state(),
        JobStateV1::Cancelled
    );
    drop(context);
    drop(runtime);
    drop(dispatcher);
    drop(runtime_manager);
    let reopened = manager(fixture.temporary_path())
        .resolve_existing(git_selector(fixture.main()), None, observation())
        .expect("committed cancellation must survive reopening the manager");
    let reopened_context = reopened.context_snapshot();
    assert_eq!(
        reopened_context
            .store()
            .read_job(
                reopened_context.binding().identity(),
                &JobId::new("00000000-0000-4000-8000-000000000911").unwrap(),
            )
            .expect("reopened Store must be readable")
            .expect("committed cancellation must be durable")
            .state(),
        podway_store::JobStateV1::Cancelled
    );
}

#[test]
fn aut_t_recon_job_lookup_projects_every_state_without_mutating_the_queue() {
    let fixture = git_worktrees();
    make_runtime_private(fixture.main());
    let runtime_manager = Arc::new(manager(fixture.temporary_path()));
    let dispatcher = compose_dispatcher_v1(
        Arc::clone(&runtime_manager),
        WorkerIdV1::new("recon-matrix-dispatcher").unwrap(),
    );
    let workspace_selector = selector(fixture.main());
    let initialize = request(
        920,
        "workspace.init",
        &workspace_selector,
        json!({"selector": serde_json::to_value(&workspace_selector).unwrap()}),
        RequestOptionsV1::new(false, 5_000).unwrap(),
        "recon-matrix-succeeded",
        PreconditionsV1::default(),
    );
    let initialized = dispatch_command(&dispatcher, &initialize, "workspace.init");
    assert_eq!(initialized.job().unwrap().state(), JobStateV1::Succeeded);
    let start = request(
        919,
        "session.start",
        &workspace_selector,
        json!({
            "selector": serde_json::to_value(&workspace_selector).unwrap(),
            "preset": "sw-dev",
            "task_title": "Retain reconciliation receipts across restart",
        }),
        RequestOptionsV1::new(false, 5_000).unwrap(),
        "recon-matrix-session",
        PreconditionsV1::default(),
    );
    let started = dispatch_command(&dispatcher, &start, "session.start");
    let session_id = started.session().unwrap().id().clone();

    let runtime = runtime_manager
        .resolve_existing(git_selector(fixture.main()), None, observation())
        .expect("initialized workspace must resolve through the manager");
    let context = runtime.context_snapshot();
    let direct_store = SqliteStoreV1::open(
        context.database_path(),
        context.workspace_root(),
        context.binding().identity().clone(),
        context.store_options().clone(),
        UnixMillis::new(10),
    )
    .expect("manager binding must reopen for deterministic reconciliation setup");

    let admit = |job_number: u64, key: &str, digest_nibble: char, now: u64| {
        let job_id = JobId::new(format!("00000000-0000-4000-8000-{job_number:012x}")).unwrap();
        let outcome = direct_store
            .admit(
                context.binding().identity(),
                AdmitRequestV1::new(
                    CommandV1::SessionComplete,
                    StoreIdempotencyKeyV1::new(key).unwrap(),
                    job_id.clone(),
                    RevisionAttemptItemPreconditionsV1::new(None, None, None, None).unwrap(),
                    Sha256Digest::new(format!("sha256:{}", digest_nibble.to_string().repeat(64)))
                        .unwrap(),
                    UnixMillis::new(now),
                )
                .with_session_identity(AdmissionSessionIdentityV1::Exact(session_id.clone()))
                .with_response_context(
                    PersistedResponseContextV1::new(
                        job_id.as_str(),
                        "session.complete",
                        context.binding().identity().workspace_uuid().clone(),
                        "/safe/worktree",
                        job_number,
                    )
                    .unwrap(),
                ),
            )
            .expect("reconciliation fixture admission must succeed");
        assert!(matches!(outcome, AdmitOutcomeV1::New(_)));
        job_id
    };

    let failed_id = admit(921, "recon-matrix-failed", '1', 11);
    let failed_claim = direct_store
        .claim_next(
            context.binding().identity(),
            WorkerIdV1::new("recon-matrix-failed-worker").unwrap(),
            UnixMillis::new(12),
        )
        .unwrap()
        .expect("failed fixture must be claimable");
    assert_eq!(failed_claim.job().job_id(), &failed_id);
    direct_store
        .commit_terminal(
            failed_claim.claim().clone(),
            Revision::ZERO,
            None,
            TerminalResultV1::Failure(DomainError::InvalidState {
                reason: "deterministic reconciliation domain failure",
            }),
            UnixMillis::new(13),
        )
        .expect("domain failure must commit a terminal failed receipt");
    let failed_before_pruning = request(
        925,
        "job.lookup",
        &workspace_selector,
        json!({
            "selector": serde_json::to_value(&workspace_selector).unwrap(),
            "idempotency_key": "recon-matrix-failed",
        }),
        RequestOptionsV1::new(false, 0).unwrap(),
        "unused-query-key",
        PreconditionsV1::default(),
    );
    let failed_before_pruning = dispatch_command(&dispatcher, &failed_before_pruning, "job.lookup");
    let failed_terminal_before_pruning =
        failed_before_pruning.result()["job"]["terminal_response"].clone();
    assert_eq!(failed_terminal_before_pruning["schema"], "podway.error/v1");

    let cancelled_id = admit(922, "recon-matrix-cancelled", '2', 14);
    direct_store
        .cancel_before_claim(
            context.binding().identity(),
            cancelled_id.clone(),
            Revision::new(4),
            UnixMillis::new(15),
        )
        .expect("queued reconciliation fixture must cancel");

    for ordinal in 0..100_u64 {
        let now = 20 + ordinal * 3;
        let filler_id = admit(
            1_000 + ordinal,
            &format!("recon-matrix-retention-{ordinal}"),
            '5',
            now,
        );
        let filler_claim = direct_store
            .claim_next(
                context.binding().identity(),
                WorkerIdV1::new("recon-matrix-retention-worker").unwrap(),
                UnixMillis::new(now + 1),
            )
            .unwrap()
            .expect("retention fixture must be claimable");
        assert_eq!(filler_claim.job().job_id(), &filler_id);
        direct_store
            .commit_terminal(
                filler_claim.claim().clone(),
                Revision::ZERO,
                None,
                TerminalResultV1::Failure(DomainError::InvalidState {
                    reason: "retention filler",
                }),
                UnixMillis::new(now + 2),
            )
            .expect("retention filler must commit");
    }

    let running_id = admit(923, "recon-matrix-running", '3', 400);
    let running_claim = direct_store
        .claim_next(
            context.binding().identity(),
            WorkerIdV1::new("recon-matrix-running-worker").unwrap(),
            UnixMillis::new(401),
        )
        .unwrap()
        .expect("running fixture must be claimable");
    assert_eq!(running_claim.job().job_id(), &running_id);
    let queued_id = admit(924, "recon-matrix-queued", '4', 402);
    drop(direct_store);
    drop(context);
    drop(runtime);
    drop(dispatcher);
    drop(runtime_manager);

    let runtime_manager = Arc::new(manager(fixture.temporary_path()));
    let dispatcher = compose_dispatcher_v1(
        Arc::clone(&runtime_manager),
        WorkerIdV1::new("recon-matrix-restarted-dispatcher").unwrap(),
    );
    let runtime = runtime_manager
        .resolve_existing(git_selector(fixture.main()), None, observation())
        .expect("reconciliation state must survive daemon restart");
    let context = runtime.context_snapshot();
    let direct_store = SqliteStoreV1::open(
        context.database_path(),
        context.workspace_root(),
        context.binding().identity().clone(),
        context.store_options().clone(),
        UnixMillis::new(500),
    )
    .expect("restarted manager binding must reopen for pruning");
    let recovered_running = direct_store
        .claim_next(
            context.binding().identity(),
            WorkerIdV1::new("recon-matrix-restarted-running-worker").unwrap(),
            UnixMillis::new(501),
        )
        .unwrap()
        .expect("restart recovery must make the interrupted job claimable");
    assert_eq!(recovered_running.job().job_id(), &running_id);

    // Startup recovery applies retention before accepting requests. These oldest terminal rows
    // must already be pruned while their session-scoped idempotency receipts remain readable.
    for job_id in [&failed_id, &cancelled_id] {
        assert!(
            direct_store
                .read_job(context.binding().identity(), job_id)
                .unwrap()
                .is_none(),
            "terminal lookup must survive without job row {job_id}"
        );
    }

    let jobs_before = direct_store
        .list_jobs(
            context.binding().identity(),
            JobListQueryV1::new(1_000).unwrap(),
        )
        .unwrap();
    drop(direct_store);

    for (index, (key, expected_state, terminal_schema)) in [
        (
            "recon-matrix-succeeded",
            "succeeded",
            Some("podway.output/v1"),
        ),
        ("recon-matrix-failed", "failed", Some("podway.error/v1")),
        ("recon-matrix-cancelled", "cancelled", Some("cancelled")),
        ("recon-matrix-running", "running", None),
        ("recon-matrix-queued", "queued", None),
    ]
    .into_iter()
    .enumerate()
    {
        let lookup = request(
            930 + index as u64,
            "job.lookup",
            &workspace_selector,
            json!({
                "selector": serde_json::to_value(&workspace_selector).unwrap(),
                "idempotency_key": key,
            }),
            RequestOptionsV1::new(false, 0).unwrap(),
            "unused-query-key",
            PreconditionsV1::default(),
        );
        let lookup = dispatch_command(&dispatcher, &lookup, "job.lookup");
        assert_eq!(lookup.result()["found"], true, "{key}");
        assert_eq!(lookup.result()["job"]["state"], expected_state, "{key}");
        assert!(
            lookup.result()["job"]["request_digest"]
                .as_str()
                .is_some_and(|digest| digest.starts_with("sha256:")),
            "{key} must expose its canonical request digest"
        );
        match terminal_schema {
            Some(schema) => {
                assert_eq!(
                    lookup.result()["job"]["terminal_response"]
                        .get("schema")
                        .unwrap_or(&lookup.result()["job"]["terminal_response"]["kind"]),
                    schema,
                    "{key}"
                );
                if key == "recon-matrix-failed" {
                    assert_eq!(
                        lookup.result()["job"]["terminal_response"],
                        failed_terminal_before_pruning,
                        "receipt-only lookup after restart must reproduce the complete original error envelope"
                    );
                    assert_eq!(
                        lookup.result()["job"]["terminal_response"]["details"]["admission"],
                        json!({
                            "admitted": true,
                            "job_id": failed_id.as_str(),
                            "workspace_sequence": 3
                        }),
                        "receipt-only failure lookup must preserve durable admission identity"
                    );
                }
            }
            None => assert!(
                lookup.result()["job"]["terminal_response"].is_null(),
                "{key}"
            ),
        }
    }

    let missing = request(
        940,
        "job.lookup",
        &workspace_selector,
        json!({
            "selector": serde_json::to_value(&workspace_selector).unwrap(),
            "idempotency_key": "recon-matrix-missing",
        }),
        RequestOptionsV1::new(false, 0).unwrap(),
        "unused-query-key",
        PreconditionsV1::default(),
    );
    let missing = dispatch_command(&dispatcher, &missing, "job.lookup");
    assert_eq!(
        missing.result(),
        &json!({"found": false}).as_object().unwrap().clone()
    );

    let jobs_after = context
        .store()
        .list_jobs(
            context.binding().identity(),
            JobListQueryV1::new(1_000).unwrap(),
        )
        .unwrap();
    assert_eq!(
        jobs_after, jobs_before,
        "lookup must not claim, retry, or cancel jobs"
    );
    assert_eq!(
        jobs_after
            .iter()
            .find(|job| job.job().job_id() == &queued_id)
            .unwrap()
            .state(),
        podway_store::JobStateV1::Queued
    );
}
