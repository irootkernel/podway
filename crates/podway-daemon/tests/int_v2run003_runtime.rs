//! Production vertical coverage for V2RUN-003 action completion and item read-back.

use crate::support_phase4_workspace;

use std::{
    fs,
    path::Path,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use podway_config::{
    AuthoringContext, ParsedProcedure, ProcedureDocumentFormat, parse_procedure_document,
    validate_procedure_v2, vet_procedure_v2,
};
use podway_core::{AttemptId, Revision, SessionId, UnixMillis};
use podway_daemon::{
    dispatch::{
        CatalogDispatchErrorMapperV1, DispatcherWorkspaceOutputV1, ProcedureV2AdmissionProofV1,
        RequestDispatcherV1Adapter, WorkspaceRuntimeV1,
    },
    production::{
        NativeProductionClockV1, ProductionControlServiceV1, ProductionMutationWorkerV1,
        ProductionPreviewServiceV1, ProductionReadServiceV1, ProductionWorkspaceRuntimeV1,
        ProductionWorkspaceV1,
    },
    runtime_workspace::{WorkspaceRuntimeManagerV1, WorkspaceRuntimeObservationV1},
    server::{DaemonRequestV1, RequestDispatcherV1},
};
use podway_protocol::{
    ClientInfoV1, CommandNameV1, IdempotencyKeyV1, OperationV1, PreconditionsV1,
    RequestEnvelopeInputV1, RequestEnvelopeV1, RequestIdV1, RequestOptionsV1, ResponseEnvelopeV2,
    Rfc3339MillisV1, SliceRequestV1, WorkspaceContextV1, WorktreeSelectorWireV1,
};
use podway_service::ServiceRuntimePathsV1;
use podway_store::{SqliteStoreOptionsV1, SqliteStoreV1, StoreGraphStateContractV2, WorkerIdV1};
use serde_json::{Map, Value, json};
use sha2::{Digest as _, Sha256};
use support_phase4_workspace::selector as git_selector;

const ACTION_READBACK_PROCEDURE: &str = include_str!("fixtures/action-readback-procedure.yaml");

pub(super) const TEST_WAIT_TIMEOUT_MILLIS: u64 = 30_000;

fn fixture_runtime_directory(root: &Path) -> std::path::PathBuf {
    let root = fs::canonicalize(root).unwrap();
    let digest = Sha256::digest(root.to_string_lossy().as_bytes());
    std::env::temp_dir().join(format!("pdr-v2run003-{}", &format!("{digest:x}")[..16]))
}

pub(super) fn manager(root: &Path) -> WorkspaceRuntimeManagerV1 {
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
    WorkspaceRuntimeManagerV1::new(&paths, SqliteStoreOptionsV1::new(8).unwrap())
}

pub(super) fn make_runtime_private(root: &Path) {
    #[cfg(unix)]
    fs::set_permissions(
        root.join(".podway/runtime"),
        fs::Permissions::from_mode(0o700),
    )
    .unwrap();
}

pub(super) fn selector(path: &Path) -> WorktreeSelectorWireV1 {
    let canonical = fs::canonicalize(path).unwrap();
    WorktreeSelectorWireV1::new(
        canonical.to_string_lossy().as_bytes(),
        canonical.display().to_string(),
        None,
    )
    .unwrap()
}

pub(super) fn observation() -> WorkspaceRuntimeObservationV1 {
    WorkspaceRuntimeObservationV1::new(
        UnixMillis::new(1_700_000_000_123),
        Rfc3339MillisV1::new("2026-07-15T12:34:56.789Z").unwrap(),
    )
}

#[derive(Clone)]
struct DevelopmentV2RoutingRuntime {
    inner: ProductionWorkspaceRuntimeV1,
}

impl WorkspaceRuntimeV1 for DevelopmentV2RoutingRuntime {
    type Workspace = ProductionWorkspaceV1;

    fn resolve_existing(
        &self,
        selector: &WorktreeSelectorWireV1,
    ) -> Result<Self::Workspace, podway_daemon::dispatch::DispatchFailureV1> {
        self.inner.resolve_existing(selector)
    }

    fn resolve_existing_readonly(
        &self,
        selector: &WorktreeSelectorWireV1,
    ) -> Result<Self::Workspace, podway_daemon::dispatch::DispatchFailureV1> {
        self.inner.resolve_existing_readonly(selector)
    }

    fn resolve_bootstrap(
        &self,
        selector: &WorktreeSelectorWireV1,
    ) -> Result<Self::Workspace, podway_daemon::dispatch::DispatchFailureV1> {
        self.inner.resolve_bootstrap(selector)
    }

    fn procedure_v2_admission(
        &self,
        _selector: &WorktreeSelectorWireV1,
    ) -> Result<Option<ProcedureV2AdmissionProofV1>, podway_daemon::dispatch::DispatchFailureV1>
    {
        Ok(Some(ProcedureV2AdmissionProofV1::granted_for_runtime()))
    }

    fn workspace_output(&self, workspace: &Self::Workspace) -> podway_protocol::WorkspaceOutputV1 {
        self.inner.workspace_output(workspace)
    }

    fn doctor(
        &self,
        selector: &WorktreeSelectorWireV1,
        deep: bool,
    ) -> Result<DispatcherWorkspaceOutputV1, podway_daemon::dispatch::DispatchFailureV1> {
        self.inner.doctor(selector, deep)
    }

    fn show(
        &self,
        selector: &WorktreeSelectorWireV1,
    ) -> Result<DispatcherWorkspaceOutputV1, podway_daemon::dispatch::DispatchFailureV1> {
        self.inner.show(selector)
    }

    fn repair(
        &self,
        selector: &WorktreeSelectorWireV1,
    ) -> Result<DispatcherWorkspaceOutputV1, podway_daemon::dispatch::DispatchFailureV1> {
        self.inner.repair(selector)
    }
}

pub(super) fn dispatcher(
    manager: Arc<WorkspaceRuntimeManagerV1>,
    worker_id: &str,
) -> impl RequestDispatcherV1 {
    let clock = Arc::new(NativeProductionClockV1::default());
    let worker = ProductionMutationWorkerV1::new(
        WorkerIdV1::new(worker_id).unwrap(),
        Arc::clone(&clock),
        Arc::clone(&manager),
    );
    RequestDispatcherV1Adapter::new(
        DevelopmentV2RoutingRuntime {
            inner: ProductionWorkspaceRuntimeV1::new(Arc::clone(&manager), Arc::clone(&clock)),
        },
        ProductionReadServiceV1::new(Arc::clone(&manager), Arc::clone(&clock)),
        ProductionControlServiceV1::new(Arc::clone(&clock)),
        ProductionPreviewServiceV1::new(Arc::clone(&clock)),
        worker,
        clock,
        CatalogDispatchErrorMapperV1,
    )
}

pub(super) fn request(
    request_number: u64,
    command: &str,
    selector: &WorktreeSelectorWireV1,
    payload: Map<String, Value>,
    idempotency_key: &str,
    preconditions: PreconditionsV1,
) -> (RequestEnvelopeV1, DaemonRequestV1) {
    request_with_options(
        request_number,
        command,
        selector,
        payload,
        idempotency_key,
        preconditions,
        RequestOptionsV1::new(false, TEST_WAIT_TIMEOUT_MILLIS).unwrap(),
    )
}

pub(super) fn dispatch_after_cold_reopen(
    dispatcher: &impl RequestDispatcherV1,
    request: &(RequestEnvelopeV1, DaemonRequestV1),
) -> ResponseEnvelopeV2 {
    let deadline = Instant::now() + Duration::from_millis(TEST_WAIT_TIMEOUT_MILLIS);
    loop {
        let response = dispatch(dispatcher, request);
        if !matches!(
            &response,
            ResponseEnvelopeV2::Error(error)
                if matches!(
                    error.code().as_str(),
                    "DAEMON_UNAVAILABLE" | "WORKSPACE_MAINTENANCE"
                )
        ) {
            return response;
        }
        assert!(
            Instant::now() < deadline,
            "cold-reopen lifecycle did not become available within {} seconds: {response:?}",
            TEST_WAIT_TIMEOUT_MILLIS / 1_000
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn request_with_options(
    request_number: u64,
    command: &str,
    selector: &WorktreeSelectorWireV1,
    mut payload: Map<String, Value>,
    idempotency_key: &str,
    preconditions: PreconditionsV1,
    options: RequestOptionsV1,
) -> (RequestEnvelopeV1, DaemonRequestV1) {
    payload.insert(
        "selector".to_owned(),
        serde_json::to_value(selector).unwrap(),
    );
    let operation = match command {
        "workspace.init" | "workspace.reset_all" => OperationV1::Bootstrap,
        "workspace.show" | "session.status" | "session.next" | "session.observe" | "job.lookup"
        | "job.status" | "job.wait" => OperationV1::Query,
        _ => OperationV1::Mutate,
    };
    let envelope = RequestEnvelopeV1::new(RequestEnvelopeInputV1 {
        request_id: RequestIdV1::new(format!("00000000-0000-4000-8000-{request_number:012x}"))
            .unwrap(),
        client: ClientInfoV1::new("v2run003-test", "1", 1).unwrap(),
        operation,
        command: CommandNameV1::new(command).unwrap(),
        workspace: Some(
            WorkspaceContextV1::new(selector.display(), selector.expected_uuid().cloned()).unwrap(),
        ),
        idempotency_key: matches!(operation, OperationV1::Bootstrap | OperationV1::Mutate)
            .then(|| IdempotencyKeyV1::new(idempotency_key).unwrap()),
        preconditions,
        options,
        payload,
    })
    .unwrap();
    let slice = SliceRequestV1::from_envelope(&envelope).unwrap();
    let daemon = DaemonRequestV1::from_envelope(&envelope).unwrap();
    assert_eq!(slice.command().command_name(), command);
    (envelope, daemon)
}

pub(super) fn dispatch(
    dispatcher: &impl RequestDispatcherV1,
    request: &(RequestEnvelopeV1, DaemonRequestV1),
) -> ResponseEnvelopeV2 {
    dispatcher.dispatch_daemon(&request.0, &request.1)
}

pub(super) fn v2_result(response: ResponseEnvelopeV2, command: &str) -> Map<String, Value> {
    let ResponseEnvelopeV2::OutputV2(output) = &response else {
        panic!(
            "{command} against a Procedure v2 session must return podway.output/v3: {response:?}"
        )
    };
    assert_eq!(output.command().as_str(), command);
    output.result().clone()
}

pub(super) fn response_request_id(response: &ResponseEnvelopeV2) -> &RequestIdV1 {
    match response {
        ResponseEnvelopeV2::OutputV2(output) => output.request_id(),
        ResponseEnvelopeV2::Error(error) => error.request_id(),
    }
}

pub(super) fn without_request_id(response: &ResponseEnvelopeV2) -> Value {
    let mut value = serde_json::to_value(response).unwrap();
    value.as_object_mut().unwrap().remove("request_id");
    value
}

pub(super) fn session_preconditions(status: &Map<String, Value>) -> PreconditionsV1 {
    PreconditionsV1::new(
        Some(SessionId::new(status["session"]["id"].as_str().unwrap()).unwrap()),
        Some(Revision::new(
            status["session"]["revision"].as_u64().unwrap(),
        )),
        Some(AttemptId::new(status["current"]["attempt"]["attempt_id"].as_str().unwrap()).unwrap()),
        None,
        None,
        None,
    )
    .unwrap()
}

pub(super) fn item_preconditions(status: &Map<String, Value>, item_id: &str) -> PreconditionsV1 {
    let item = status["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|item| item["item_id"] == item_id)
        .unwrap();
    PreconditionsV1::new(
        Some(SessionId::new(status["session"]["id"].as_str().unwrap()).unwrap()),
        None,
        Some(AttemptId::new(status["current"]["attempt"]["attempt_id"].as_str().unwrap()).unwrap()),
        Some(Revision::new(item["revision"].as_u64().unwrap())),
        None,
        None,
    )
    .unwrap()
}

pub(super) fn status(
    dispatcher: &impl RequestDispatcherV1,
    selector: &WorktreeSelectorWireV1,
    request_number: u64,
    expected_session_id: &str,
) -> Map<String, Value> {
    let request = request(
        request_number,
        "session.status",
        selector,
        Map::new(),
        "unused-status-key",
        PreconditionsV1::new(
            Some(SessionId::new(expected_session_id).unwrap()),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap(),
    );
    v2_result(dispatch(dispatcher, &request), "session.status")
}

#[allow(clippy::too_many_arguments)]
pub(super) fn mutate_item(
    dispatcher: &impl RequestDispatcherV1,
    selector: &WorktreeSelectorWireV1,
    request_number: u64,
    session_id: &str,
    command: &str,
    item_id: &str,
    mut fields: Map<String, Value>,
    key: &str,
) -> (Map<String, Value>, ResponseEnvelopeV2) {
    let before = status(dispatcher, selector, request_number, session_id);
    fields.insert("item_id".to_owned(), json!(item_id));
    let mutation = request(
        request_number + 1,
        command,
        selector,
        fields,
        key,
        item_preconditions(&before, item_id),
    );
    let response = dispatch(dispatcher, &mutation);
    let result = v2_result(response.clone(), command);
    assert_eq!(result["schema"], "podway.item-mutation-result/v2");
    assert_eq!(
        result["graph_node_id"],
        before["current"]["node"]["graph_node_id"]
    );
    assert_eq!(result["item_id"], item_id);
    (result, response)
}

#[test]
fn v2run003_action_readback_fixture_is_valid_and_vetted() {
    let ParsedProcedure::V2(parsed) = parse_procedure_document(
        ACTION_READBACK_PROCEDURE.as_bytes(),
        ProcedureDocumentFormat::Yaml,
    )
    .unwrap() else {
        panic!("the V2RUN-003 runtime fixture must be Procedure v2")
    };
    let validated = validate_procedure_v2(parsed).unwrap();
    let context = AuthoringContext::new(
        "action-readback-procedure.yaml",
        ACTION_READBACK_PROCEDURE,
        ProcedureDocumentFormat::Yaml,
    );
    let diagnostics = vet_procedure_v2(&validated, &context);
    assert!(
        diagnostics.is_empty(),
        "the V2RUN-003 runtime fixture must pass structural vetting: {diagnostics:?}"
    );
}

#[test]
fn v2run003_detached_job_wait_reads_terminal_v2_job_from_v2_only_store() {
    let fixture = support_phase4_workspace::git_worktrees();
    make_runtime_private(fixture.main());
    fs::write(
        fixture.main().join("detached-job-wait.yaml"),
        ACTION_READBACK_PROCEDURE,
    )
    .unwrap();
    fs::write(fixture.main().join("report.txt"), b"durable report\n").unwrap();
    let ParsedProcedure::V2(parsed) = parse_procedure_document(
        ACTION_READBACK_PROCEDURE.as_bytes(),
        ProcedureDocumentFormat::Yaml,
    )
    .unwrap() else {
        unreachable!()
    };
    let procedure_digest = validate_procedure_v2(parsed).unwrap().digest().clone();
    let workspace_selector = selector(fixture.main());
    let runtime_manager = Arc::new(manager(fixture.temporary_path()));
    let production = dispatcher(Arc::clone(&runtime_manager), "v2run003-detached-job-wait");

    let initialize = request(
        29_001,
        "workspace.init",
        &workspace_selector,
        Map::new(),
        "v2run003-detached-initialize",
        PreconditionsV1::default(),
    );
    assert!(matches!(
        dispatch(&production, &initialize),
        ResponseEnvelopeV2::OutputV2(_)
    ));

    let start = request(
        29_002,
        "session.start",
        &workspace_selector,
        json!({
            "procedure": "detached-job-wait.yaml",
            "expected_procedure_digest": procedure_digest,
            "task_title": "Detached Procedure v2 job wait"
        })
        .as_object()
        .unwrap()
        .clone(),
        "v2run003-detached-start",
        PreconditionsV1::default(),
    );
    let started = v2_result(dispatch(&production, &start), "session.start");
    let session_id = started["session_id"].as_str().unwrap();
    let before_retry = status(&production, &workspace_selector, 29_003, session_id);
    let retry = request_with_options(
        29_004,
        "session.retry",
        &workspace_selector,
        json!({"reason": "prove detached Procedure v2 job wait"})
            .as_object()
            .unwrap()
            .clone(),
        "v2run003-detached-retry",
        session_preconditions(&before_retry),
        RequestOptionsV1::new(true, 5_000).unwrap(),
    );
    let retry_response = dispatch(&production, &retry);
    let ResponseEnvelopeV2::OutputV2(detached) = retry_response else {
        panic!("detached Procedure v2 retry must return podway.output/v3: {retry_response:?}")
    };
    let job_id = detached
        .job()
        .expect("detached mutation returns its durable job")
        .id()
        .clone();

    let status_request = request(
        29_005,
        "job.status",
        &workspace_selector,
        json!({"job_id": job_id}).as_object().unwrap().clone(),
        "unused-detached-status",
        PreconditionsV1::default(),
    );
    assert!(
        !matches!(
            dispatch(&production, &status_request),
            ResponseEnvelopeV2::Error(_)
        ),
        "job.status must read a detached Procedure v2 job before or after completion"
    );
    let lookup_request = request(
        29_007,
        "job.lookup",
        &workspace_selector,
        json!({"idempotency_key": "v2run003-detached-retry"})
            .as_object()
            .unwrap()
            .clone(),
        "unused-detached-lookup",
        PreconditionsV1::default(),
    );
    let lookup = v2_result(dispatch(&production, &lookup_request), "job.lookup");
    assert_eq!(lookup["found"], true);
    assert_eq!(lookup["job"]["id"], job_id.as_str());
    assert_eq!(lookup["job"]["command"], "session.retry");

    let wait_request = request(
        29_006,
        "job.wait",
        &workspace_selector,
        json!({"job_id": job_id}).as_object().unwrap().clone(),
        "unused-detached-wait",
        PreconditionsV1::default(),
    );
    let ResponseEnvelopeV2::OutputV2(waited) = dispatch(&production, &wait_request) else {
        panic!("terminal Procedure v2 job.wait must return podway.output/v3")
    };
    assert_eq!(waited.command().as_str(), "job.wait");
    assert_eq!(waited.job().unwrap().id(), &job_id);
    assert_eq!(waited.result()["schema"], "podway.job-result/v3");
    assert_eq!(
        serde_json::to_value(waited.job().unwrap()).unwrap()["state"],
        "succeeded"
    );
    assert_ne!(waited.result()["job"], Value::Null);
    let terminal_lookup = v2_result(dispatch(&production, &lookup_request), "job.lookup");
    assert_eq!(terminal_lookup["job"]["state"], "succeeded");
    assert_ne!(terminal_lookup["job"]["terminal_response"], Value::Null);
}

#[test]
fn v2rel003_detached_start_lookup_and_terminal_replay_use_common_automation_pipeline() {
    let fixture = support_phase4_workspace::git_worktrees();
    make_runtime_private(fixture.main());
    fs::write(
        fixture.main().join("detached-start.yaml"),
        ACTION_READBACK_PROCEDURE,
    )
    .unwrap();
    fs::write(fixture.main().join("report.txt"), b"durable report\n").unwrap();
    let ParsedProcedure::V2(parsed) = parse_procedure_document(
        ACTION_READBACK_PROCEDURE.as_bytes(),
        ProcedureDocumentFormat::Yaml,
    )
    .unwrap() else {
        unreachable!()
    };
    let procedure_digest = validate_procedure_v2(parsed).unwrap().digest().clone();
    let selector = selector(fixture.main());
    let manager = Arc::new(manager(fixture.temporary_path()));
    let production = dispatcher(Arc::clone(&manager), "v2rel003-detached-start");
    let initialize = request(
        29_101,
        "workspace.init",
        &selector,
        Map::new(),
        "v2rel003-detached-start-initialize",
        PreconditionsV1::default(),
    );
    assert!(matches!(
        dispatch(&production, &initialize),
        ResponseEnvelopeV2::OutputV2(_)
    ));

    let payload = json!({
        "procedure": "detached-start.yaml",
        "expected_procedure_digest": procedure_digest,
        "task_title": "Detached Procedure v2 start"
    })
    .as_object()
    .unwrap()
    .clone();
    let detached_start = request_with_options(
        29_102,
        "session.start",
        &selector,
        payload.clone(),
        "v2rel003-detached-start",
        PreconditionsV1::default(),
        RequestOptionsV1::new(true, 5_000).unwrap(),
    );
    let detached_response = dispatch(&production, &detached_start);
    let ResponseEnvelopeV2::OutputV2(detached) = detached_response else {
        panic!("detached Procedure v2 start must return podway.output/v3: {detached_response:?}")
    };
    assert_eq!(
        detached.result()["schema"],
        "podway.detached-admission-result/v2"
    );
    let job_id = detached.job().unwrap().id().clone();

    let lookup = request(
        29_103,
        "job.lookup",
        &selector,
        json!({"idempotency_key": "v2rel003-detached-start"})
            .as_object()
            .unwrap()
            .clone(),
        "unused-v2rel003-detached-start-lookup",
        PreconditionsV1::default(),
    );
    let queued_lookup = v2_result(dispatch(&production, &lookup), "job.lookup");
    assert_eq!(queued_lookup["found"], true);
    assert_eq!(queued_lookup["job"]["id"], job_id.as_str());
    assert_eq!(queued_lookup["job"]["command"], "session.start");
    assert!(matches!(
        queued_lookup["job"]["state"].as_str(),
        Some("queued" | "running" | "succeeded")
    ));

    let synchronous = request_with_options(
        29_104,
        "session.start",
        &selector,
        payload.clone(),
        "v2rel003-detached-start",
        PreconditionsV1::default(),
        RequestOptionsV1::new(false, 5_000).unwrap(),
    );
    let terminal = dispatch(&production, &synchronous);
    let terminal_result = v2_result(terminal.clone(), "session.start");
    assert_eq!(terminal_result["schema"], "podway.session-start-result/v2");
    assert_eq!(terminal_result["admission"]["job_id"], job_id.as_str());

    fs::remove_file(fixture.main().join("detached-start.yaml")).unwrap();
    let replay = request_with_options(
        29_105,
        "session.start",
        &selector,
        payload,
        "v2rel003-detached-start",
        PreconditionsV1::default(),
        RequestOptionsV1::new(false, 5_000).unwrap(),
    );
    let replayed = dispatch(&production, &replay);
    assert_eq!(
        without_request_id(&replayed),
        without_request_id(&terminal),
        "start replay must preserve the frozen terminal receipt after source deletion"
    );

    let terminal_lookup = v2_result(dispatch(&production, &lookup), "job.lookup");
    assert_eq!(terminal_lookup["job"]["state"], "succeeded");
    assert_eq!(terminal_lookup["job"]["id"], job_id.as_str());
    assert_ne!(terminal_lookup["job"]["terminal_response"], Value::Null);
}

#[test]
fn v2run003_production_actions_mutate_complete_read_back_restart_and_replay() {
    let fixture = support_phase4_workspace::git_worktrees();
    make_runtime_private(fixture.main());
    fs::write(
        fixture.main().join("action-readback.yaml"),
        ACTION_READBACK_PROCEDURE,
    )
    .unwrap();
    fs::write(fixture.main().join("report.txt"), b"durable report\n").unwrap();
    let ParsedProcedure::V2(parsed) = parse_procedure_document(
        ACTION_READBACK_PROCEDURE.as_bytes(),
        ProcedureDocumentFormat::Yaml,
    )
    .unwrap() else {
        unreachable!()
    };
    let procedure_digest = validate_procedure_v2(parsed).unwrap().digest().clone();
    let workspace_selector = selector(fixture.main());
    let runtime_manager = Arc::new(manager(fixture.temporary_path()));
    let production = dispatcher(Arc::clone(&runtime_manager), "v2run003-production");

    let initialize = request(
        30_001,
        "workspace.init",
        &workspace_selector,
        Map::new(),
        "v2run003-initialize",
        PreconditionsV1::default(),
    );
    assert!(matches!(
        dispatch(&production, &initialize),
        ResponseEnvelopeV2::OutputV2(_)
    ));

    let start = request(
        30_002,
        "session.start",
        &workspace_selector,
        json!({
            "procedure": "action-readback.yaml",
            "expected_procedure_digest": procedure_digest,
            "task_title": "V2RUN-003 production runtime"
        })
        .as_object()
        .unwrap()
        .clone(),
        "v2run003-start",
        PreconditionsV1::default(),
    );
    let started = v2_result(dispatch(&production, &start), "session.start");
    let session_id = started["session_id"].as_str().unwrap().to_owned();

    let _ = status(&production, &workspace_selector, 30_003, &session_id);

    mutate_item(
        &production,
        &workspace_selector,
        30_010,
        &session_id,
        "item.check",
        "confirmed",
        Map::new(),
        "check-confirmed",
    );
    mutate_item(
        &production,
        &workspace_selector,
        30_020,
        &session_id,
        "item.uncheck",
        "confirmed",
        Map::new(),
        "uncheck-confirmed",
    );
    mutate_item(
        &production,
        &workspace_selector,
        30_030,
        &session_id,
        "item.check",
        "confirmed",
        Map::new(),
        "recheck-confirmed",
    );
    for (number, item_id, value) in [
        (30_040, "summary", "selected summary"),
        (30_050, "outcome", "accepted"),
        (30_060, "count", "7"),
        (30_070, "internal-note", "must stay private"),
    ] {
        mutate_item(
            &production,
            &workspace_selector,
            number,
            &session_id,
            "item.set",
            item_id,
            json!({"value": value}).as_object().unwrap().clone(),
            &format!("set-{item_id}"),
        );
    }
    mutate_item(
        &production,
        &workspace_selector,
        30_080,
        &session_id,
        "item.add",
        "findings",
        json!({"value": "discarded"}).as_object().unwrap().clone(),
        "add-discarded",
    );
    mutate_item(
        &production,
        &workspace_selector,
        30_090,
        &session_id,
        "item.add",
        "findings",
        json!({"value": "retained"}).as_object().unwrap().clone(),
        "add-retained",
    );
    mutate_item(
        &production,
        &workspace_selector,
        30_100,
        &session_id,
        "item.remove",
        "findings",
        json!({"value": "discarded", "ignore_missing": false})
            .as_object()
            .unwrap()
            .clone(),
        "remove-discarded",
    );
    mutate_item(
        &production,
        &workspace_selector,
        30_110,
        &session_id,
        "item.attach",
        "report",
        json!({"path": "report.txt", "media_type": "text/plain"})
            .as_object()
            .unwrap()
            .clone(),
        "attach-report",
    );
    mutate_item(
        &production,
        &workspace_selector,
        30_120,
        &session_id,
        "item.clear",
        "summary",
        Map::new(),
        "clear-summary",
    );

    let incomplete = status(&production, &workspace_selector, 30_130, &session_id);
    let rejected_complete = request(
        30_131,
        "session.complete",
        &workspace_selector,
        Map::new(),
        "complete-missing-summary",
        session_preconditions(&incomplete),
    );
    let ResponseEnvelopeV2::Error(error) = dispatch(&production, &rejected_complete) else {
        panic!("an action with a missing required item must reject completion")
    };
    assert_eq!(error.code().as_str(), "REQUIRED_ITEMS_MISSING");

    mutate_item(
        &production,
        &workspace_selector,
        30_140,
        &session_id,
        "item.set",
        "summary",
        json!({"value": "selected summary"})
            .as_object()
            .unwrap()
            .clone(),
        "restore-summary",
    );

    let ready = status(&production, &workspace_selector, 30_150, &session_id);
    fs::remove_file(fixture.main().join("report.txt")).unwrap();
    let missing_artifact_complete = request(
        30_148,
        "session.complete",
        &workspace_selector,
        Map::new(),
        "complete-missing-artifact",
        session_preconditions(&ready),
    );
    let missing_artifact_once = dispatch(&production, &missing_artifact_complete);
    let ResponseEnvelopeV2::Error(missing_artifact_error) = &missing_artifact_once else {
        panic!("completion must reject a missing required local artifact")
    };
    assert_eq!(missing_artifact_error.code().as_str(), "ARTIFACT_CHANGED");
    let missing_artifact_replay_request = request(
        30_154,
        "session.complete",
        &workspace_selector,
        Map::new(),
        "complete-missing-artifact",
        session_preconditions(&ready),
    );
    let missing_artifact_replay = dispatch(&production, &missing_artifact_replay_request);
    assert_eq!(
        response_request_id(&missing_artifact_replay),
        missing_artifact_replay_request.0.request_id()
    );
    assert_eq!(
        without_request_id(&missing_artifact_replay),
        without_request_id(&missing_artifact_once)
    );
    let after_missing_artifact = status(&production, &workspace_selector, 30_155, &session_id);
    assert_eq!(
        after_missing_artifact["session"]["revision"],
        ready["session"]["revision"]
    );
    fs::write(fixture.main().join("report.txt"), b"durable report\n").unwrap();

    fs::write(fixture.main().join("report.txt"), b"changed report\n").unwrap();
    let changed_artifact_complete = request(
        30_149,
        "session.complete",
        &workspace_selector,
        Map::new(),
        "complete-changed-artifact",
        session_preconditions(&ready),
    );
    let changed_artifact_once = dispatch(&production, &changed_artifact_complete);
    let ResponseEnvelopeV2::Error(changed_artifact_error) = &changed_artifact_once else {
        panic!("completion must revalidate required local artifacts")
    };
    assert_eq!(changed_artifact_error.code().as_str(), "ARTIFACT_CHANGED");
    assert_eq!(
        serde_json::to_value(dispatch(&production, &changed_artifact_complete)).unwrap(),
        serde_json::to_value(changed_artifact_once).unwrap(),
        "an admitted artifact failure must replay its frozen error exactly"
    );
    let after_artifact_failure = status(&production, &workspace_selector, 30_152, &session_id);
    assert_eq!(
        after_artifact_failure["session"]["revision"],
        ready["session"]["revision"]
    );
    assert_eq!(
        after_artifact_failure["current"]["attempt"]["attempt_id"],
        ready["current"]["attempt"]["attempt_id"]
    );
    fs::write(fixture.main().join("report.txt"), b"durable report\n").unwrap();

    let complete_capture = request(
        30_151,
        "session.complete",
        &workspace_selector,
        Map::new(),
        "complete-capture",
        session_preconditions(&ready),
    );
    let completed_once = dispatch(&production, &complete_capture);
    let completed = v2_result(completed_once.clone(), "session.complete");
    assert_eq!(completed["schema"], "podway.stage-transition-result/v2");
    assert_eq!(completed["from_graph_node_id"], "capture-results");
    assert_eq!(completed["to_graph_node_id"], "consume-results");
    assert_eq!(completed["session_state"], "running");
    let replay_complete_capture = request(
        30_153,
        "session.complete",
        &workspace_selector,
        Map::new(),
        "complete-capture",
        session_preconditions(&ready),
    );
    let completed_replay = dispatch(&production, &replay_complete_capture);
    assert_eq!(
        response_request_id(&completed_replay),
        replay_complete_capture.0.request_id(),
        "a direct idempotent replay must correlate to the retry request"
    );
    assert_eq!(
        without_request_id(&completed_replay),
        without_request_id(&completed_once),
        "a direct replay may change only its transport request correlation"
    );

    let runtime = runtime_manager
        .resolve_existing(git_selector(fixture.main()), None, observation())
        .unwrap();
    let context = runtime.context_snapshot();
    let store = SqliteStoreV1::open(
        context.database_path(),
        context.workspace_root(),
        context.binding().identity().clone(),
        context.store_options().clone(),
        UnixMillis::new(1),
    )
    .unwrap();
    let graph = store
        .read_graph_session_v2(context.binding().identity())
        .unwrap()
        .unwrap();
    let complete_source_digest = graph.workflow_memory().attempts()[0]
        .recorded_items_digest()
        .unwrap();
    drop(store);

    let next = request(
        30_160,
        "session.next",
        &workspace_selector,
        Map::new(),
        "unused-next-key",
        PreconditionsV1::new(
            Some(SessionId::new(&session_id).unwrap()),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap(),
    );
    let next_before_restart = v2_result(dispatch(&production, &next), "session.next");
    let readback = next_before_restart["readback"].as_array().unwrap();
    assert_eq!(readback.len(), 2);
    let selected_ids = readback[0]["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["item_id"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(selected_ids, ["summary", "findings", "report"]);
    assert_eq!(readback[0]["items_digest"], complete_source_digest.as_str());
    assert_eq!(readback[1]["source_graph_node_id"], "finish");
    assert_eq!(readback[1]["state"], "unresolved");
    assert!(readback[1]["items"].as_array().unwrap().is_empty());

    let observe = request(
        30_161,
        "session.observe",
        &workspace_selector,
        Map::new(),
        "unused-observe-key",
        PreconditionsV1::new(
            Some(SessionId::new(&session_id).unwrap()),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap(),
    );
    let observation = v2_result(dispatch(&production, &observe), "session.observe");
    assert_eq!(observation["schema"], "podway.observation-result/v1");
    assert_eq!(observation["status"]["session"]["id"], session_id);
    assert_eq!(
        observation["guidance"]["readback"],
        next_before_restart["readback"]
    );
    assert_eq!(observation["active_items"].as_array().unwrap().len(), 1);
    assert!(
        observation["mutation_templates"]
            .as_array()
            .unwrap()
            .iter()
            .all(|template| template["idempotency_key_required"] == true)
    );

    fs::remove_file(fixture.main().join("action-readback.yaml")).unwrap();
    fs::remove_file(fixture.main().join("report.txt")).unwrap();
    drop(production);
    drop(runtime);
    drop(runtime_manager);

    let restarted_manager = Arc::new(manager(fixture.temporary_path()));
    let restarted = dispatcher(Arc::clone(&restarted_manager), "v2run003-restarted");
    let next_after_restart = v2_result(dispatch(&restarted, &next), "session.next");
    assert_eq!(
        next_after_restart["readback"],
        next_before_restart["readback"]
    );

    mutate_item(
        &restarted,
        &workspace_selector,
        30_170,
        &session_id,
        "item.check",
        "consumed",
        Map::new(),
        "check-consumed",
    );
    let consume_ready = status(&restarted, &workspace_selector, 30_180, &session_id);
    let complete_consume = request(
        30_181,
        "session.complete",
        &workspace_selector,
        Map::new(),
        "complete-consume",
        session_preconditions(&consume_ready),
    );
    let consumed = v2_result(dispatch(&restarted, &complete_consume), "session.complete");
    assert_eq!(consumed["to_graph_node_id"], "finish");
    assert_eq!(consumed["session_state"], "running");

    let finish_ready = status(&restarted, &workspace_selector, 30_190, &session_id);
    let complete_finish = request(
        30_191,
        "session.complete",
        &workspace_selector,
        Map::new(),
        "complete-finish",
        session_preconditions(&finish_ready),
    );
    let finished = v2_result(dispatch(&restarted, &complete_finish), "session.complete");
    assert_eq!(finished["from_graph_node_id"], "finish");
    assert_eq!(finished["session_state"], "completed");
}
