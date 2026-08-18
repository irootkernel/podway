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
    let operation = if payload.get("dry_run").and_then(Value::as_bool) == Some(true) {
        OperationV1::Query
    } else {
        match command {
            "workspace.init" | "workspace.reset_all" => OperationV1::Bootstrap,
            "workspace.show" | "session.status" | "session.next" | "session.observe"
            | "job.list" | "job.lookup" | "job.status" | "job.wait" => OperationV1::Query,
            _ => OperationV1::Mutate,
        }
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
    let daemon = DaemonRequestV1::from_envelope(&envelope).unwrap();
    if matches!(command, "session.begin" | "session.terminal_disposition") {
        assert!(matches!(daemon, DaemonRequestV1::ProcedureV2Mutation(_)));
    } else {
        let slice = SliceRequestV1::from_envelope(&envelope).unwrap();
        assert_eq!(slice.command().command_name(), command);
    }
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

pub(super) fn begin(
    dispatcher: &impl RequestDispatcherV1,
    selector: &WorktreeSelectorWireV1,
    request_number: u64,
    session_id: &str,
    payload: Map<String, Value>,
    idempotency_key: &str,
) -> Map<String, Value> {
    let request = request(
        request_number,
        "session.begin",
        selector,
        payload,
        idempotency_key,
        PreconditionsV1::new(
            Some(SessionId::new(session_id).unwrap()),
            Some(Revision::ZERO),
            None,
            None,
            None,
            None,
        )
        .unwrap(),
    );
    v2_result(dispatch(dispatcher, &request), "session.begin")
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
fn v2agt004_record_many_updates_all_item_types_atomically_and_replays_terminal_receipt() {
    let fixture = support_phase4_workspace::git_worktrees();
    make_runtime_private(fixture.main());
    fs::write(
        fixture.main().join("record-many.yaml"),
        ACTION_READBACK_PROCEDURE,
    )
    .unwrap();
    fs::write(fixture.main().join("report.txt"), b"atomic report\n").unwrap();
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
    let production = dispatcher(Arc::clone(&runtime_manager), "v2agt004-record-many");

    let initialize = request(
        28_001,
        "workspace.init",
        &workspace_selector,
        Map::new(),
        "v2agt004-initialize",
        PreconditionsV1::default(),
    );
    assert!(matches!(
        dispatch(&production, &initialize),
        ResponseEnvelopeV2::OutputV2(_)
    ));
    let start = request(
        28_002,
        "session.start",
        &workspace_selector,
        json!({
            "procedure": "record-many.yaml",
            "expected_procedure_digest": procedure_digest,
            "task_title": "V2AGT-004 atomic recording"
        })
        .as_object()
        .unwrap()
        .clone(),
        "v2agt004-start",
        PreconditionsV1::default(),
    );
    let started = v2_result(dispatch(&production, &start), "session.start");
    let session_id = started["session_id"].as_str().unwrap();
    begin(
        &production,
        &workspace_selector,
        28_099,
        session_id,
        Map::new(),
        "v2agt004-begin",
    );
    let before = status(&production, &workspace_selector, 28_003, session_id);
    let revision_before = before["session"]["revision"].as_u64().unwrap();
    let preconditions = session_preconditions(&before);
    let batch = request(
        28_004,
        "item.record_many",
        &workspace_selector,
        json!({"operations":[
            {"item_id":"summary","expected_item_revision":0,"record":{"type":"text","value":"atomic summary"}},
            {"item_id":"report","expected_item_revision":0,"record":{"type":"artifact","path":"report.txt","media_type":"text/plain"}},
            {"item_id":"outcome","expected_item_revision":0,"record":{"type":"choice","value":"accepted"}},
            {"item_id":"findings","expected_item_revision":0,"record":{"type":"list","value":["one","two"]}},
            {"item_id":"count","expected_item_revision":0,"record":{"type":"integer","value":7}},
            {"item_id":"confirmed","expected_item_revision":0,"record":{"type":"confirm","value":true}}
        ]})
        .as_object()
        .unwrap()
        .clone(),
        "v2agt004-batch",
        preconditions.clone(),
    );
    let response = dispatch(&production, &batch);
    let result = v2_result(response.clone(), "item.record_many");
    assert_eq!(result["schema"], "podway.item-record-many-result/v1");
    assert_eq!(result["changed"], true);
    assert_eq!(result["revision"], revision_before + 1);
    assert_eq!(result["items"].as_array().unwrap().len(), 6);
    assert_eq!(result["items"][0]["item_id"], "confirmed");
    assert_eq!(result["items"][5]["item_id"], "summary");

    let after = status(&production, &workspace_selector, 28_005, session_id);
    assert_eq!(after["session"]["revision"], revision_before + 1);
    for item_id in [
        "confirmed",
        "summary",
        "outcome",
        "count",
        "findings",
        "report",
    ] {
        assert!(
            after["items"]
                .as_array()
                .unwrap()
                .iter()
                .find(|item| item["item_id"] == item_id)
                .is_some_and(|item| item["satisfied"] == true)
        );
    }

    assert_eq!(
        without_request_id(&dispatch(&production, &batch)),
        without_request_id(&response),
        "exact idempotent replay must return the sealed terminal response"
    );

    let invalid = request(
        28_006,
        "item.record_many",
        &workspace_selector,
        json!({"operations":[
            {"item_id":"confirmed","expected_item_revision":1,"clear":true},
            {"item_id":"summary","expected_item_revision":0,"clear":true}
        ]})
        .as_object()
        .unwrap()
        .clone(),
        "v2agt004-invalid-item-revision",
        session_preconditions(&after),
    );
    let ResponseEnvelopeV2::Error(error) = dispatch(&production, &invalid) else {
        panic!("a batch with one stale item revision must fail atomically")
    };
    assert_eq!(error.code().as_str(), "ITEM_REVISION_CONFLICT");
    let unchanged_after_invalid = status(&production, &workspace_selector, 28_007, session_id);
    assert_eq!(
        unchanged_after_invalid["session"]["revision"],
        revision_before + 1
    );
    assert!(
        unchanged_after_invalid["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["item_id"] == "confirmed")
            .is_some_and(|item| item["satisfied"] == true)
    );

    let stale = request(
        28_008,
        "item.record_many",
        &workspace_selector,
        json!({"operations":[
            {"item_id":"summary","expected_item_revision":0,"clear":true}
        ]})
        .as_object()
        .unwrap()
        .clone(),
        "v2agt004-stale",
        preconditions,
    );
    let ResponseEnvelopeV2::Error(error) = dispatch(&production, &stale) else {
        panic!("a stale batch must fail before changing any item")
    };
    assert_eq!(error.code().as_str(), "SESSION_REVISION_CONFLICT");
    let unchanged = status(&production, &workspace_selector, 28_009, session_id);
    assert_eq!(unchanged["session"]["revision"], revision_before + 1);
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
    begin(
        &production,
        &workspace_selector,
        29_099,
        session_id,
        Map::new(),
        "v2run003-detached-begin",
    );
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
    assert_eq!(waited.result()["schema"], "podway.job-result/v4");
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
fn v2lif007_workspace_and_lifecycle_jobs_remain_readable_after_cold_reopen() {
    let fixture = support_phase4_workspace::git_worktrees();
    make_runtime_private(fixture.main());
    fs::write(
        fixture.main().join("v2lif007-readback.yaml"),
        ACTION_READBACK_PROCEDURE,
    )
    .unwrap();
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
    let production = dispatcher(Arc::clone(&runtime_manager), "v2lif007-readback");

    let initialize = request(
        29_051,
        "workspace.init",
        &workspace_selector,
        Map::new(),
        "v2lif007-initialize",
        PreconditionsV1::default(),
    );
    let ResponseEnvelopeV2::OutputV2(initialized) = dispatch(&production, &initialize) else {
        panic!("workspace.init must succeed");
    };
    let initialize_job_id = initialized.job().unwrap().id().clone();

    let start = request(
        29_052,
        "session.start",
        &workspace_selector,
        json!({
            "procedure": "v2lif007-readback.yaml",
            "expected_procedure_digest": procedure_digest,
            "task_title": "Read every durable lifecycle job"
        })
        .as_object()
        .unwrap()
        .clone(),
        "v2lif007-start",
        PreconditionsV1::default(),
    );
    let ResponseEnvelopeV2::OutputV2(started) = dispatch(&production, &start) else {
        panic!("session.start must prepare the session");
    };
    let start_job_id = started.job().unwrap().id().clone();
    let session_id = started.result()["session_id"].as_str().unwrap();

    let begin = request(
        29_053,
        "session.begin",
        &workspace_selector,
        Map::new(),
        "v2lif007-begin",
        PreconditionsV1::new(
            Some(SessionId::new(session_id).unwrap()),
            Some(Revision::ZERO),
            None,
            None,
            None,
            None,
        )
        .unwrap(),
    );
    let ResponseEnvelopeV2::OutputV2(began) = dispatch(&production, &begin) else {
        panic!("session.begin must start the prepared session");
    };
    let begin_job_id = began.job().unwrap().id().clone();

    let list = request(
        29_054,
        "job.list",
        &workspace_selector,
        Map::new(),
        "unused-v2lif007-list",
        PreconditionsV1::default(),
    );
    let listed = v2_result(dispatch(&production, &list), "job.list");
    let jobs = listed["jobs"].as_array().unwrap();
    assert_eq!(jobs.len(), 3);
    assert_eq!(
        jobs.iter()
            .map(|job| job["command"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["workspace.init", "session.start", "session.begin"]
    );
    assert!(
        jobs.iter()
            .all(|job| job["terminal_response"] != Value::Null)
    );

    for (request_number, job_id, command) in [
        (29_055, &initialize_job_id, "workspace.init"),
        (29_056, &start_job_id, "session.start"),
        (29_057, &begin_job_id, "session.begin"),
    ] {
        let status = request(
            request_number,
            "job.status",
            &workspace_selector,
            json!({"job_id": job_id}).as_object().unwrap().clone(),
            "unused-v2lif007-status",
            PreconditionsV1::default(),
        );
        let status = v2_result(dispatch(&production, &status), "job.status");
        assert_eq!(status["schema"], "podway.job-result/v4");
        assert_eq!(status["job"]["command"], command);
    }

    drop(production);
    drop(runtime_manager);
    let restarted_manager = Arc::new(manager(fixture.temporary_path()));
    let restarted = dispatcher(Arc::clone(&restarted_manager), "v2lif007-restarted");
    let reopened = v2_result(dispatch_after_cold_reopen(&restarted, &list), "job.list");
    assert_eq!(reopened, listed);

    let init_status = request(
        29_058,
        "job.status",
        &workspace_selector,
        json!({"job_id": initialize_job_id})
            .as_object()
            .unwrap()
            .clone(),
        "unused-v2lif007-reopened-status",
        PreconditionsV1::default(),
    );
    let reopened_init = v2_result(
        dispatch_after_cold_reopen(&restarted, &init_status),
        "job.status",
    );
    assert_eq!(reopened_init["job"]["command"], "workspace.init");
}

#[test]
fn v2lif008_prepared_item_mutations_fail_before_attempt_and_item_fences() {
    let fixture = support_phase4_workspace::git_worktrees();
    make_runtime_private(fixture.main());
    fs::write(
        fixture.main().join("v2lif008-prepared-items.yaml"),
        ACTION_READBACK_PROCEDURE,
    )
    .unwrap();
    fs::write(fixture.main().join("report.txt"), b"prepared report\n").unwrap();
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
    let production = dispatcher(Arc::clone(&runtime_manager), "v2lif008-prepared-items");

    let initialize = request(
        29_061,
        "workspace.init",
        &workspace_selector,
        Map::new(),
        "v2lif008-initialize",
        PreconditionsV1::default(),
    );
    assert!(matches!(
        dispatch(&production, &initialize),
        ResponseEnvelopeV2::OutputV2(_)
    ));
    let start = request(
        29_062,
        "session.start",
        &workspace_selector,
        json!({
            "procedure": "v2lif008-prepared-items.yaml",
            "expected_procedure_digest": procedure_digest,
            "task_title": "Reject prepared item mutations"
        })
        .as_object()
        .unwrap()
        .clone(),
        "v2lif008-start",
        PreconditionsV1::default(),
    );
    let started = v2_result(dispatch(&production, &start), "session.start");
    let session_id = SessionId::new(started["session_id"].as_str().unwrap()).unwrap();
    let prepared_before = status(
        &production,
        &workspace_selector,
        29_063,
        session_id.as_str(),
    );
    let stale_attempt = AttemptId::new("00000000-0000-4000-8000-000000000099").unwrap();

    let mutations = [
        ("item.check", json!({"item_id":"confirmed"})),
        ("item.uncheck", json!({"item_id":"confirmed"})),
        ("item.set", json!({"item_id":"summary","value":"prepared"})),
        ("item.add", json!({"item_id":"findings","value":"prepared"})),
        (
            "item.remove",
            json!({"item_id":"findings","value":"prepared","ignore_missing":false}),
        ),
        (
            "item.attach",
            json!({"item_id":"report","path":"report.txt","media_type":"text/plain"}),
        ),
        ("item.clear", json!({"item_id":"summary"})),
    ];
    for (offset, (command, payload)) in mutations.into_iter().enumerate() {
        let mutation = request(
            29_064 + u64::try_from(offset).unwrap(),
            command,
            &workspace_selector,
            payload.as_object().unwrap().clone(),
            &format!("v2lif008-prepared-{command}"),
            PreconditionsV1::new(
                Some(session_id.clone()),
                None,
                Some(stale_attempt.clone()),
                Some(Revision::new(99)),
                None,
                None,
            )
            .unwrap(),
        );
        let ResponseEnvelopeV2::Error(error) = dispatch(&production, &mutation) else {
            panic!("{command} must fail before durable admission in prepared lifecycle");
        };
        assert_eq!(error.code().as_str(), "SESSION_NOT_RUNNING", "{command}");
        assert_eq!(error.details()["admission"]["admitted"], false, "{command}");
    }

    let record_many = request(
        29_071,
        "item.record_many",
        &workspace_selector,
        json!({"operations":[{
            "item_id":"confirmed",
            "expected_item_revision":99,
            "record":{"type":"confirm","value":true}
        }]})
        .as_object()
        .unwrap()
        .clone(),
        "v2lif008-prepared-record-many",
        PreconditionsV1::new(
            Some(session_id.clone()),
            Some(Revision::ZERO),
            Some(stale_attempt.clone()),
            None,
            None,
            None,
        )
        .unwrap(),
    );
    let ResponseEnvelopeV2::Error(error) = dispatch(&production, &record_many) else {
        panic!("item.record_many must fail before durable admission in prepared lifecycle");
    };
    assert_eq!(error.code().as_str(), "SESSION_NOT_RUNNING");
    assert_eq!(error.details()["admission"]["admitted"], false);

    let prepared_after = status(
        &production,
        &workspace_selector,
        29_072,
        session_id.as_str(),
    );
    assert_eq!(prepared_after, prepared_before);
    let list = request(
        29_073,
        "job.list",
        &workspace_selector,
        Map::new(),
        "unused-v2lif008-list",
        PreconditionsV1::default(),
    );
    assert_eq!(
        v2_result(dispatch(&production, &list), "job.list")["jobs"]
            .as_array()
            .unwrap()
            .len(),
        2
    );

    begin(
        &production,
        &workspace_selector,
        29_074,
        session_id.as_str(),
        Map::new(),
        "v2lif008-begin",
    );
    for (request_number, command, payload, session_revision, item_revision) in [
        (
            29_075,
            "item.check",
            json!({"item_id":"confirmed"}),
            None,
            Some(Revision::new(99)),
        ),
        (
            29_076,
            "item.record_many",
            json!({"operations":[{
                "item_id":"confirmed",
                "expected_item_revision":99,
                "record":{"type":"confirm","value":true}
            }]}),
            Some(Revision::new(1)),
            None,
        ),
    ] {
        let mutation = request(
            request_number,
            command,
            &workspace_selector,
            payload.as_object().unwrap().clone(),
            &format!("v2lif008-running-{command}"),
            PreconditionsV1::new(
                Some(session_id.clone()),
                session_revision,
                Some(stale_attempt.clone()),
                item_revision,
                None,
                None,
            )
            .unwrap(),
        );
        let ResponseEnvelopeV2::Error(error) = dispatch(&production, &mutation) else {
            panic!("{command} must preserve its running stale-attempt fence");
        };
        assert_eq!(error.code().as_str(), "ATTEMPT_NOT_CURRENT", "{command}");
        assert_eq!(error.details()["admission"]["admitted"], false, "{command}");
    }
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
    assert_eq!(terminal_result["schema"], "podway.session-start-result/v3");
    assert_eq!(terminal_result["session_state"], "prepared");
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
    begin(
        &production,
        &workspace_selector,
        30_009,
        &session_id,
        Map::new(),
        "v2run003-begin",
    );

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
    assert_eq!(observation["schema"], "podway.observation-result/v2");
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

#[test]
fn v2lif004_prepared_begin_disposition_and_eligible_reset_form_one_runtime_flow() {
    let fixture = support_phase4_workspace::git_worktrees();
    make_runtime_private(fixture.main());
    fs::write(
        fixture.main().join("lifecycle.yaml"),
        ACTION_READBACK_PROCEDURE,
    )
    .unwrap();
    let ParsedProcedure::V2(parsed) = parse_procedure_document(
        ACTION_READBACK_PROCEDURE.as_bytes(),
        ProcedureDocumentFormat::Yaml,
    )
    .unwrap();
    let digest = validate_procedure_v2(parsed).unwrap().digest().clone();
    let selector = selector(fixture.main());
    let manager = Arc::new(manager(fixture.temporary_path()));
    let production = dispatcher(Arc::clone(&manager), "v2lif004-runtime");

    let initialize = request(
        140_001,
        "workspace.init",
        &selector,
        Map::new(),
        "v2lif004-init",
        PreconditionsV1::default(),
    );
    assert!(matches!(
        dispatch(&production, &initialize),
        ResponseEnvelopeV2::OutputV2(_)
    ));

    let start = request(
        140_002,
        "session.start",
        &selector,
        json!({
            "procedure": "lifecycle.yaml",
            "expected_procedure_digest": digest,
            "task_title": "Exercise prepared lifecycle"
        })
        .as_object()
        .unwrap()
        .clone(),
        "v2lif004-start",
        PreconditionsV1::default(),
    );
    let started = v2_result(dispatch(&production, &start), "session.start");
    assert_eq!(started["schema"], "podway.session-start-result/v3");
    assert_eq!(started["session_state"], "prepared");
    assert_eq!(started["revision"], 0);
    assert!(started["active_attempt"].is_null());
    let session_id = SessionId::new(started["session_id"].as_str().unwrap()).unwrap();

    let prepared = status(&production, &selector, 140_003, session_id.as_str());
    assert_eq!(prepared["session"]["lifecycle"], "prepared");
    assert!(prepared["current"].is_null());

    let identity_fence = PreconditionsV1::new(
        Some(session_id.clone()),
        Some(Revision::ZERO),
        None,
        None,
        None,
        None,
    )
    .unwrap();
    let stale_begin = request(
        149_997,
        "session.begin",
        &selector,
        Map::new(),
        "v2lif004-begin-stale-revision",
        PreconditionsV1::new(
            Some(session_id.clone()),
            Some(Revision::new(1)),
            None,
            None,
            None,
            None,
        )
        .unwrap(),
    );
    let ResponseEnvelopeV2::Error(stale_begin) = dispatch(&production, &stale_begin) else {
        panic!("begin must reject a stale prepared-session revision");
    };
    assert_eq!(stale_begin.code().as_str(), "SESSION_REVISION_CONFLICT");

    let mismatched_begin = request(
        149_996,
        "session.begin",
        &selector,
        Map::new(),
        "v2lif004-begin-session-mismatch",
        PreconditionsV1::new(
            Some(SessionId::new("00000000-0000-4000-8000-000000149996").unwrap()),
            Some(Revision::ZERO),
            None,
            None,
            None,
            None,
        )
        .unwrap(),
    );
    let ResponseEnvelopeV2::Error(mismatched_begin) = dispatch(&production, &mismatched_begin)
    else {
        panic!("begin must reject a mismatched prepared session");
    };
    assert_eq!(mismatched_begin.code().as_str(), "SESSION_ID_MISMATCH");

    let cancel_prepared = request(
        149_999,
        "session.cancel",
        &selector,
        json!({"reason":"Prepared sessions have no active work to cancel."})
            .as_object()
            .unwrap()
            .clone(),
        "v2lif004-cancel-prepared",
        PreconditionsV1::new(
            Some(session_id.clone()),
            Some(Revision::ZERO),
            Some(AttemptId::new("00000000-0000-4000-8000-000000014999").unwrap()),
            None,
            None,
            None,
        )
        .unwrap(),
    );
    let ResponseEnvelopeV2::Error(cancel_prepared) = dispatch(&production, &cancel_prepared) else {
        panic!("cancel must reject a prepared session");
    };
    assert_eq!(cancel_prepared.code().as_str(), "SESSION_NOT_RUNNING");

    let begin = request(
        140_004,
        "session.begin",
        &selector,
        Map::new(),
        "v2lif004-begin",
        identity_fence,
    );
    let begun = v2_result(dispatch(&production, &begin), "session.begin");
    assert_eq!(begun["schema"], "podway.session-begin-result/v1");
    assert_eq!(begun["session_state"], "running");
    assert_eq!(begun["revision"], 1);
    assert_eq!(begun["goal_defined"], false);

    let begin_again = request(
        149_998,
        "session.begin",
        &selector,
        Map::new(),
        "v2lif004-begin-running",
        PreconditionsV1::new(
            Some(session_id.clone()),
            Some(Revision::new(1)),
            None,
            None,
            None,
            None,
        )
        .unwrap(),
    );
    let ResponseEnvelopeV2::Error(begin_again) = dispatch(&production, &begin_again) else {
        panic!("begin must reject a running session");
    };
    assert_eq!(begin_again.code().as_str(), "SESSION_NOT_RUNNING");

    let running = status(&production, &selector, 140_005, session_id.as_str());
    assert_eq!(running["session"]["revision"], 1);
    let running_identity_fence = PreconditionsV1::new(
        Some(session_id.clone()),
        Some(Revision::new(
            running["session"]["revision"].as_u64().unwrap(),
        )),
        None,
        None,
        None,
        None,
    )
    .unwrap();
    let rejected_disposition = request(
        140_006,
        "session.terminal_disposition",
        &selector,
        json!({"kind":"not_required","reason":"Running is not terminal."})
            .as_object()
            .unwrap()
            .clone(),
        "v2lif004-running-disposition",
        running_identity_fence.clone(),
    );
    let ResponseEnvelopeV2::Error(rejected_disposition) =
        dispatch(&production, &rejected_disposition)
    else {
        panic!("terminal disposition must reject a running session");
    };
    assert_eq!(rejected_disposition.code().as_str(), "SESSION_NOT_TERMINAL");

    let rejected_reset = request(
        140_007,
        "session.reset",
        &selector,
        Map::new(),
        "v2lif004-running-reset",
        running_identity_fence,
    );
    let ResponseEnvelopeV2::Error(rejected_reset) = dispatch(&production, &rejected_reset) else {
        panic!("eligible reset must reject a running session");
    };
    assert_eq!(rejected_reset.code().as_str(), "SESSION_RESET_NOT_ELIGIBLE");

    let cancel = request(
        140_008,
        "session.cancel",
        &selector,
        json!({"reason":"Exercise terminal disposition."})
            .as_object()
            .unwrap()
            .clone(),
        "v2lif004-cancel",
        session_preconditions(&running),
    );
    let cancelled = v2_result(dispatch(&production, &cancel), "session.cancel");
    assert_eq!(cancelled["session_state"], "cancelled");
    let terminal_revision = cancelled["revision"].as_u64().unwrap();

    let terminal_fence = PreconditionsV1::new(
        Some(session_id.clone()),
        Some(Revision::new(terminal_revision)),
        None,
        None,
        None,
        None,
    )
    .unwrap();
    let stale_disposition = request(
        149_995,
        "session.terminal_disposition",
        &selector,
        json!({"kind":"not_required","reason":"A stale terminal fence must fail."})
            .as_object()
            .unwrap()
            .clone(),
        "v2lif004-disposition-stale-revision",
        PreconditionsV1::new(
            Some(session_id.clone()),
            Some(Revision::new(terminal_revision - 1)),
            None,
            None,
            None,
            None,
        )
        .unwrap(),
    );
    let ResponseEnvelopeV2::Error(stale_disposition) = dispatch(&production, &stale_disposition)
    else {
        panic!("terminal disposition must reject a stale session revision");
    };
    assert_eq!(
        stale_disposition.code().as_str(),
        "SESSION_REVISION_CONFLICT"
    );

    let disposition = request(
        140_009,
        "session.terminal_disposition",
        &selector,
        json!({
            "kind":"handed_off",
            "summary":"Delivered the prepared lifecycle result.",
            "reference":"commit:v2lif005",
            "actor":"integration-test"
        })
        .as_object()
        .unwrap()
        .clone(),
        "v2lif004-disposition",
        terminal_fence.clone(),
    );
    let disposed = v2_result(
        dispatch(&production, &disposition),
        "session.terminal_disposition",
    );
    assert_eq!(disposed["schema"], "podway.terminal-disposition-result/v1");
    assert_eq!(disposed["disposition"]["kind"], "handed_off");
    assert_eq!(
        disposed["disposition"]["summary"],
        "Delivered the prepared lifecycle result."
    );
    assert_eq!(disposed["disposition"]["reference"], "commit:v2lif005");
    assert_eq!(disposed["disposition"]["actor"], "integration-test");

    let duplicate_disposition = request(
        140_010,
        "session.terminal_disposition",
        &selector,
        json!({"kind":"not_required","reason":"A second assertion is forbidden."})
            .as_object()
            .unwrap()
            .clone(),
        "v2lif004-duplicate-disposition",
        terminal_fence.clone(),
    );
    let ResponseEnvelopeV2::Error(duplicate_disposition) =
        dispatch(&production, &duplicate_disposition)
    else {
        panic!("one terminal revision must admit only one immutable disposition");
    };
    assert_eq!(duplicate_disposition.code().as_str(), "REQUEST_INVALID");

    let stale_duplicate_disposition = request(
        140_012,
        "session.terminal_disposition",
        &selector,
        json!({"kind":"not_required","reason":"The stale fence must win."})
            .as_object()
            .unwrap()
            .clone(),
        "v2lif005-duplicate-disposition-stale-revision",
        PreconditionsV1::new(
            Some(session_id.clone()),
            Some(Revision::new(terminal_revision - 1)),
            None,
            None,
            None,
            None,
        )
        .unwrap(),
    );
    let ResponseEnvelopeV2::Error(stale_duplicate_disposition) =
        dispatch(&production, &stale_duplicate_disposition)
    else {
        panic!("a stale disposition fence must fail before duplicate-domain validation");
    };
    assert_eq!(
        stale_duplicate_disposition.code().as_str(),
        "SESSION_REVISION_CONFLICT"
    );

    let reset = request(
        140_011,
        "session.reset",
        &selector,
        Map::new(),
        "v2lif004-reset",
        terminal_fence,
    );
    let reset = v2_result(dispatch(&production, &reset), "session.reset");
    assert_eq!(reset["schema"], "podway.session-reset-result/v1");
    assert_eq!(reset["mode"], "eligible");
    assert_eq!(reset["reset"], true);
}

#[test]
fn v2lif005_prepared_session_dry_run_and_eligible_reset_need_no_force_summary() {
    let fixture = support_phase4_workspace::git_worktrees();
    make_runtime_private(fixture.main());
    fs::write(
        fixture.main().join("prepared-reset.yaml"),
        ACTION_READBACK_PROCEDURE,
    )
    .unwrap();
    let ParsedProcedure::V2(parsed) = parse_procedure_document(
        ACTION_READBACK_PROCEDURE.as_bytes(),
        ProcedureDocumentFormat::Yaml,
    )
    .unwrap();
    let digest = validate_procedure_v2(parsed).unwrap().digest().clone();
    let selector = selector(fixture.main());
    let manager = Arc::new(manager(fixture.temporary_path()));
    let production = dispatcher(Arc::clone(&manager), "v2lif005-prepared-reset");

    let initialize = request(
        142_001,
        "workspace.init",
        &selector,
        Map::new(),
        "v2lif005-prepared-reset-init",
        PreconditionsV1::default(),
    );
    assert!(matches!(
        dispatch(&production, &initialize),
        ResponseEnvelopeV2::OutputV2(_)
    ));
    let start = request(
        142_002,
        "session.start",
        &selector,
        json!({
            "procedure": "prepared-reset.yaml",
            "expected_procedure_digest": digest,
            "task_title": "Reset unused preparation"
        })
        .as_object()
        .unwrap()
        .clone(),
        "v2lif005-prepared-reset-start",
        PreconditionsV1::default(),
    );
    let started = v2_result(dispatch(&production, &start), "session.start");
    let session_id = SessionId::new(started["session_id"].as_str().unwrap()).unwrap();
    let prepared_fence = PreconditionsV1::new(
        Some(session_id.clone()),
        Some(Revision::ZERO),
        None,
        None,
        None,
        None,
    )
    .unwrap();

    let goal_begin = request(
        142_006,
        "session.begin",
        &selector,
        json!({
            "goal":"Bind a goal where goal tracking is disabled.",
            "criteria":[{
                "criterion_id":"verified",
                "statement":"The invalid goal is rejected."
            }]
        })
        .as_object()
        .unwrap()
        .clone(),
        "v2lif005-prepared-reset-goal-begin",
        prepared_fence.clone(),
    );
    let ResponseEnvelopeV2::Error(goal_begin) = dispatch(&production, &goal_begin) else {
        panic!("goal-bearing begin must reject a goal-free procedure");
    };
    assert_eq!(goal_begin.code().as_str(), "GOAL_TRACKING_NOT_ENABLED");
    let still_prepared = status(&production, &selector, 142_007, session_id.as_str());
    assert_eq!(still_prepared["session"]["lifecycle"], "prepared");
    assert_eq!(still_prepared["session"]["revision"], 0);

    let dry_run = request(
        142_003,
        "session.reset",
        &selector,
        json!({"dry_run":true}).as_object().unwrap().clone(),
        "v2lif005-prepared-reset-dry-run",
        prepared_fence.clone(),
    );
    let dry_run = v2_result(dispatch(&production, &dry_run), "session.reset");
    assert_eq!(dry_run["schema"], "podway.session-reset-result/v1");
    assert_eq!(dry_run["mode"], "eligible");
    assert_eq!(dry_run["lifecycle"], "prepared");
    assert_eq!(dry_run["eligible"], true);
    assert_eq!(dry_run["required_action"], "none");
    assert_eq!(dry_run["reset"], false);

    let reset = request(
        142_004,
        "session.reset",
        &selector,
        Map::new(),
        "v2lif005-prepared-reset-eligible",
        prepared_fence,
    );
    let reset = v2_result(dispatch(&production, &reset), "session.reset");
    assert_eq!(reset["schema"], "podway.session-reset-result/v1");
    assert_eq!(reset["mode"], "eligible");
    assert_eq!(reset["lifecycle"], "prepared");
    assert_eq!(reset["eligible"], true);
    assert_eq!(reset["reset"], true);

    let status_after_reset = request(
        142_005,
        "session.status",
        &selector,
        Map::new(),
        "unused-status-key",
        PreconditionsV1::new(Some(session_id), None, None, None, None, None).unwrap(),
    );
    let ResponseEnvelopeV2::Error(status_after_reset) = dispatch(&production, &status_after_reset)
    else {
        panic!("eligible reset must remove the prepared session");
    };
    assert_eq!(status_after_reset.code().as_str(), "SESSION_ID_MISMATCH");
}

#[test]
fn v2lif003_cold_read_migrates_released_v4_and_rebuilds_missing_registry_once() {
    let fixture = support_phase4_workspace::git_worktrees();
    make_runtime_private(fixture.main());
    fs::write(
        fixture.main().join("v2lif003-cold-reactivation.yaml"),
        ACTION_READBACK_PROCEDURE,
    )
    .unwrap();
    let ParsedProcedure::V2(parsed) = parse_procedure_document(
        ACTION_READBACK_PROCEDURE.as_bytes(),
        ProcedureDocumentFormat::Yaml,
    )
    .unwrap() else {
        unreachable!()
    };
    let digest = validate_procedure_v2(parsed).unwrap().digest().clone();
    let workspace_selector = selector(fixture.main());
    let original_manager = Arc::new(manager(fixture.temporary_path()));
    let original = dispatcher(Arc::clone(&original_manager), "v2lif003-original");

    let initialize = request(
        143_001,
        "workspace.init",
        &workspace_selector,
        Map::new(),
        "v2lif003-initialize",
        PreconditionsV1::default(),
    );
    assert!(matches!(
        dispatch(&original, &initialize),
        ResponseEnvelopeV2::OutputV2(_)
    ));
    let start = request(
        143_002,
        "session.start",
        &workspace_selector,
        json!({
            "procedure": "v2lif003-cold-reactivation.yaml",
            "expected_procedure_digest": digest,
            "task_title": "Cold-reactivate released state"
        })
        .as_object()
        .unwrap()
        .clone(),
        "v2lif003-start",
        PreconditionsV1::default(),
    );
    let started = v2_result(dispatch(&original, &start), "session.start");
    let session_id = started["session_id"].as_str().unwrap().to_owned();
    begin(
        &original,
        &workspace_selector,
        143_003,
        &session_id,
        Map::new(),
        "v2lif003-begin",
    );
    let expected_status = status(&original, &workspace_selector, 143_004, &session_id);
    let scheduler = original_manager
        .resolve_existing(git_selector(fixture.main()), None, observation())
        .unwrap();
    let workspace_identity = scheduler.context_snapshot().binding().identity().clone();
    let database_path = fixture.main().join(".podway/runtime/state.sqlite3");
    drop(scheduler);
    drop(original);
    drop(original_manager);

    podway_store::test_support::downgrade_to_schema_v4(&database_path).unwrap();
    assert!(
        SqliteStoreV1::inspect_workspace_migration_required(
            &database_path,
            &workspace_identity,
            &SqliteStoreOptionsV1::new(8).unwrap(),
        )
        .unwrap()
    );

    let cold_root = fixture.temporary_path().join("v2lif003-cold-account");
    let cold_manager = Arc::new(manager(&cold_root));
    let registry_path = cold_manager.registry().registry_path().to_path_buf();
    assert!(!registry_path.exists());
    let first_request = request(
        143_005,
        "session.status",
        &workspace_selector,
        Map::new(),
        "unused-v2lif003-first",
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
    let second_request = request(
        143_006,
        "session.status",
        &workspace_selector,
        Map::new(),
        "unused-v2lif003-second",
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
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let first = {
        let manager = Arc::clone(&cold_manager);
        let barrier = Arc::clone(&barrier);
        thread::spawn(move || {
            let production = dispatcher(manager, "v2lif003-cold-first");
            barrier.wait();
            v2_result(
                dispatch_after_cold_reopen(&production, &first_request),
                "session.status",
            )
        })
    };
    let second = {
        let manager = Arc::clone(&cold_manager);
        let barrier = Arc::clone(&barrier);
        thread::spawn(move || {
            let production = dispatcher(manager, "v2lif003-cold-second");
            barrier.wait();
            v2_result(
                dispatch_after_cold_reopen(&production, &second_request),
                "session.status",
            )
        })
    };
    assert_eq!(first.join().unwrap(), expected_status);
    assert_eq!(second.join().unwrap(), expected_status);
    assert!(
        cold_manager
            .registry()
            .lookup(workspace_identity.workspace_uuid())
            .unwrap()
            .is_some()
    );
    assert!(registry_path.exists());
    assert!(
        !SqliteStoreV1::inspect_workspace_migration_required(
            &database_path,
            &workspace_identity,
            &SqliteStoreOptionsV1::new(8).unwrap(),
        )
        .unwrap()
    );
    drop(cold_manager);

    let current_root = fixture.temporary_path().join("v2lif003-current-account");
    let current_manager = Arc::new(manager(&current_root));
    let current_registry_path = current_manager.registry().registry_path().to_path_buf();
    let current = dispatcher(Arc::clone(&current_manager), "v2lif003-current-readonly");
    assert_eq!(
        status(&current, &workspace_selector, 143_007, &session_id),
        expected_status
    );
    assert!(
        !current_registry_path.exists(),
        "a current-schema cold read must not rebuild registry metadata"
    );
}

#[test]
fn v2lif007_migrated_eligible_reset_keeps_terminal_job_readback() {
    let fixture = support_phase4_workspace::git_worktrees();
    make_runtime_private(fixture.main());
    fs::write(
        fixture.main().join("v2lif007-reset-readback.yaml"),
        ACTION_READBACK_PROCEDURE,
    )
    .unwrap();
    let ParsedProcedure::V2(parsed) = parse_procedure_document(
        ACTION_READBACK_PROCEDURE.as_bytes(),
        ProcedureDocumentFormat::Yaml,
    )
    .unwrap() else {
        unreachable!()
    };
    let digest = validate_procedure_v2(parsed).unwrap().digest().clone();
    let workspace_selector = selector(fixture.main());
    let original_manager = Arc::new(manager(fixture.temporary_path()));
    let original = dispatcher(Arc::clone(&original_manager), "v2lif007-reset-original");

    let initialize = request(
        143_101,
        "workspace.init",
        &workspace_selector,
        Map::new(),
        "v2lif007-reset-init",
        PreconditionsV1::default(),
    );
    assert!(matches!(
        dispatch(&original, &initialize),
        ResponseEnvelopeV2::OutputV2(_)
    ));
    let start_payload = |title: &str| {
        json!({
            "procedure": "v2lif007-reset-readback.yaml",
            "expected_procedure_digest": digest,
            "task_title": title
        })
        .as_object()
        .unwrap()
        .clone()
    };
    let reset_source = request(
        143_102,
        "session.start",
        &workspace_selector,
        start_payload("Create a released reset receipt"),
        "v2lif007-reset-source",
        PreconditionsV1::default(),
    );
    let reset_source = v2_result(dispatch(&original, &reset_source), "session.start");
    let reset_session_id = SessionId::new(reset_source["session_id"].as_str().unwrap()).unwrap();
    let reset = request(
        143_103,
        "session.reset",
        &workspace_selector,
        Map::new(),
        "v2lif007-reset-terminal",
        PreconditionsV1::new(
            Some(reset_session_id),
            Some(Revision::ZERO),
            None,
            None,
            None,
            None,
        )
        .unwrap(),
    );
    let ResponseEnvelopeV2::OutputV2(reset_output) = dispatch(&original, &reset) else {
        panic!("eligible reset must succeed");
    };
    assert_eq!(reset_output.result()["mode"], "eligible");
    let reset_job_id = reset_output.job().unwrap().id().clone();
    let reset_terminal = serde_json::to_value(&reset_output).unwrap();

    let retained_start = request(
        143_104,
        "session.start",
        &workspace_selector,
        start_payload("Retain a running session across migration"),
        "v2lif007-retained-start",
        PreconditionsV1::default(),
    );
    let retained_start = v2_result(dispatch(&original, &retained_start), "session.start");
    let retained_session_id = retained_start["session_id"].as_str().unwrap().to_owned();
    begin(
        &original,
        &workspace_selector,
        143_105,
        &retained_session_id,
        Map::new(),
        "v2lif007-retained-begin",
    );
    let expected_status = status(
        &original,
        &workspace_selector,
        143_106,
        &retained_session_id,
    );
    let database_path = fixture.main().join(".podway/runtime/state.sqlite3");
    drop(original);
    drop(original_manager);

    podway_store::test_support::downgrade_to_schema_v4(&database_path).unwrap();
    podway_store::test_support::rewrite_reset_terminal_without_mode(&database_path, &reset_job_id)
        .unwrap();

    let cold_manager = Arc::new(manager(
        &fixture.temporary_path().join("v2lif007-reset-cold"),
    ));
    let cold = dispatcher(Arc::clone(&cold_manager), "v2lif007-reset-cold");
    assert_eq!(
        status(&cold, &workspace_selector, 143_107, &retained_session_id,),
        expected_status
    );

    let job_status = request(
        143_108,
        "job.status",
        &workspace_selector,
        json!({"job_id": reset_job_id}).as_object().unwrap().clone(),
        "unused-v2lif007-reset-status",
        PreconditionsV1::default(),
    );
    let job_status = v2_result(dispatch_after_cold_reopen(&cold, &job_status), "job.status");
    assert_eq!(job_status["job"], reset_terminal);

    let lookup = request(
        143_109,
        "job.lookup",
        &workspace_selector,
        json!({"idempotency_key": "v2lif007-reset-terminal"})
            .as_object()
            .unwrap()
            .clone(),
        "unused-v2lif007-reset-lookup",
        PreconditionsV1::default(),
    );
    let lookup = v2_result(dispatch_after_cold_reopen(&cold, &lookup), "job.lookup");
    assert_eq!(lookup["found"], true);
    assert_eq!(lookup["job"]["terminal_response"], reset_terminal);

    let list = request(
        143_110,
        "job.list",
        &workspace_selector,
        Map::new(),
        "unused-v2lif007-reset-list",
        PreconditionsV1::default(),
    );
    let list = v2_result(dispatch_after_cold_reopen(&cold, &list), "job.list");
    let listed = list["jobs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|job| job["id"] == reset_job_id.as_str())
        .expect("the migrated reset job must remain listed");
    assert_eq!(listed["terminal_response"], reset_terminal);
}

#[test]
fn v2lif005_eligible_replacement_dry_run_uses_current_reset_eligibility() {
    let fixture = support_phase4_workspace::git_worktrees();
    make_runtime_private(fixture.main());
    fs::write(
        fixture.main().join("replacement-preview.yaml"),
        ACTION_READBACK_PROCEDURE,
    )
    .unwrap();
    let ParsedProcedure::V2(parsed) = parse_procedure_document(
        ACTION_READBACK_PROCEDURE.as_bytes(),
        ProcedureDocumentFormat::Yaml,
    )
    .unwrap();
    let digest = validate_procedure_v2(parsed).unwrap().digest().clone();
    let selector = selector(fixture.main());
    let manager = Arc::new(manager(fixture.temporary_path()));
    let production = dispatcher(Arc::clone(&manager), "v2lif005-replacement-preview");

    let initialize = request(
        143_001,
        "workspace.init",
        &selector,
        Map::new(),
        "v2lif005-replacement-preview-init",
        PreconditionsV1::default(),
    );
    assert!(matches!(
        dispatch(&production, &initialize),
        ResponseEnvelopeV2::OutputV2(_)
    ));

    let start_payload = |title: &str| {
        json!({
            "procedure": "replacement-preview.yaml",
            "expected_procedure_digest": digest,
            "task_title": title
        })
        .as_object()
        .unwrap()
        .clone()
    };
    let preview = |request_number, title, preconditions| {
        let mut payload = start_payload(title);
        payload.insert("replace_eligible".to_owned(), Value::Bool(true));
        payload.insert("dry_run".to_owned(), Value::Bool(true));
        let preview = request(
            request_number,
            "session.start_replace",
            &selector,
            payload,
            "unused-replacement-preview-key",
            preconditions,
        );
        dispatch(&production, &preview)
    };
    let ResponseEnvelopeV2::Error(absent) = preview(
        143_002,
        "Absent preview",
        PreconditionsV1::new(
            Some(SessionId::new("00000000-0000-4000-8000-000000143002").unwrap()),
            Some(Revision::ZERO),
            None,
            None,
            None,
            None,
        )
        .unwrap(),
    ) else {
        panic!("eligible replacement preview must reject an absent current session");
    };
    assert_eq!(absent.code().as_str(), "SESSION_ID_MISMATCH");

    let start = request(
        143_003,
        "session.start",
        &selector,
        start_payload("Prepared preview"),
        "v2lif005-replacement-preview-start",
        PreconditionsV1::default(),
    );
    let started = v2_result(dispatch(&production, &start), "session.start");
    let session_id = SessionId::new(started["session_id"].as_str().unwrap()).unwrap();
    let prepared_fence = PreconditionsV1::new(
        Some(session_id.clone()),
        Some(Revision::ZERO),
        None,
        None,
        None,
        None,
    )
    .unwrap();

    let prepared = v2_result(
        preview(
            143_004,
            "Prepared replacement preview",
            prepared_fence.clone(),
        ),
        "session.start_replace",
    );
    assert_eq!(prepared["dry_run"], true);

    let begin = request(
        143_005,
        "session.begin",
        &selector,
        Map::new(),
        "v2lif005-replacement-preview-begin",
        prepared_fence,
    );
    let begun = v2_result(dispatch(&production, &begin), "session.begin");
    assert_eq!(begun["session_state"], "running");
    let running = status(&production, &selector, 143_050, session_id.as_str());
    let running_fence = session_preconditions(&running);

    let ResponseEnvelopeV2::Error(running) = preview(
        143_006,
        "Running replacement preview",
        PreconditionsV1::new(
            Some(session_id.clone()),
            Some(Revision::new(1)),
            None,
            None,
            None,
            None,
        )
        .unwrap(),
    ) else {
        panic!("eligible replacement preview must reject a running session");
    };
    assert_eq!(running.code().as_str(), "SESSION_RESET_NOT_ELIGIBLE");

    let cancel = request(
        143_007,
        "session.cancel",
        &selector,
        json!({"reason":"Exercise undisposed terminal preview."})
            .as_object()
            .unwrap()
            .clone(),
        "v2lif005-replacement-preview-cancel",
        running_fence,
    );
    let cancelled = v2_result(dispatch(&production, &cancel), "session.cancel");
    let terminal_revision = cancelled["revision"].as_u64().unwrap();
    let terminal_fence = PreconditionsV1::new(
        Some(session_id.clone()),
        Some(Revision::new(terminal_revision)),
        None,
        None,
        None,
        None,
    )
    .unwrap();

    let ResponseEnvelopeV2::Error(undisposed) = preview(
        143_008,
        "Undisposed replacement preview",
        terminal_fence.clone(),
    ) else {
        panic!("eligible replacement preview must reject an undisposed terminal session");
    };
    assert_eq!(undisposed.code().as_str(), "SESSION_RESET_NOT_ELIGIBLE");

    let disposition = request(
        143_009,
        "session.terminal_disposition",
        &selector,
        json!({"kind":"not_required","reason":"The preview needs no external handoff."})
            .as_object()
            .unwrap()
            .clone(),
        "v2lif005-replacement-preview-disposition",
        terminal_fence.clone(),
    );
    let _ = v2_result(
        dispatch(&production, &disposition),
        "session.terminal_disposition",
    );

    let disposed = v2_result(
        preview(143_010, "Disposed replacement preview", terminal_fence),
        "session.start_replace",
    );
    assert_eq!(disposed["dry_run"], true);
}

#[test]
fn v2lif004_eligible_and_force_replacement_then_force_reset_are_atomic() {
    let fixture = support_phase4_workspace::git_worktrees();
    make_runtime_private(fixture.main());
    fs::write(
        fixture.main().join("replacement.yaml"),
        ACTION_READBACK_PROCEDURE,
    )
    .unwrap();
    let ParsedProcedure::V2(parsed) = parse_procedure_document(
        ACTION_READBACK_PROCEDURE.as_bytes(),
        ProcedureDocumentFormat::Yaml,
    )
    .unwrap();
    let digest = validate_procedure_v2(parsed).unwrap().digest().clone();
    let selector = selector(fixture.main());
    let manager = Arc::new(manager(fixture.temporary_path()));
    let production = dispatcher(Arc::clone(&manager), "v2lif004-replacement");

    let initialize = request(
        141_001,
        "workspace.init",
        &selector,
        Map::new(),
        "v2lif004-replacement-init",
        PreconditionsV1::default(),
    );
    assert!(matches!(
        dispatch(&production, &initialize),
        ResponseEnvelopeV2::OutputV2(_)
    ));

    let start_payload = |title: &str| {
        json!({
            "procedure": "replacement.yaml",
            "expected_procedure_digest": digest,
            "task_title": title
        })
        .as_object()
        .unwrap()
        .clone()
    };
    let start = request(
        141_002,
        "session.start",
        &selector,
        start_payload("Prepared A"),
        "v2lif004-replacement-start",
        PreconditionsV1::default(),
    );
    let started = v2_result(dispatch(&production, &start), "session.start");
    let first_session = SessionId::new(started["session_id"].as_str().unwrap()).unwrap();

    let mut eligible_payload = start_payload("Prepared B");
    eligible_payload.insert("replace_eligible".to_owned(), Value::Bool(true));
    let eligible_replace = request(
        141_003,
        "session.start_replace",
        &selector,
        eligible_payload,
        "v2lif004-eligible-replace",
        PreconditionsV1::new(
            Some(first_session.clone()),
            Some(Revision::ZERO),
            None,
            None,
            None,
            None,
        )
        .unwrap(),
    );
    let replaced = v2_result(
        dispatch(&production, &eligible_replace),
        "session.start_replace",
    );
    assert_eq!(replaced["session_state"], "prepared");
    let second_session = SessionId::new(replaced["session_id"].as_str().unwrap()).unwrap();
    assert_ne!(second_session, first_session);

    let begin = request(
        141_004,
        "session.begin",
        &selector,
        Map::new(),
        "v2lif004-replacement-begin",
        PreconditionsV1::new(
            Some(second_session.clone()),
            Some(Revision::ZERO),
            None,
            None,
            None,
            None,
        )
        .unwrap(),
    );
    let begun = v2_result(dispatch(&production, &begin), "session.begin");
    assert_eq!(begun["session_state"], "running");

    let mut force_payload = start_payload("Prepared C");
    force_payload.insert("confirmed".to_owned(), Value::Bool(true));
    force_payload.insert(
        "progress_summary".to_owned(),
        Value::String("The runtime test intentionally discards its isolated work.".to_owned()),
    );
    let force_replace = request(
        141_005,
        "session.start_replace",
        &selector,
        force_payload,
        "v2lif004-force-replace",
        PreconditionsV1::new(
            Some(second_session),
            Some(Revision::new(1)),
            None,
            None,
            None,
            None,
        )
        .unwrap(),
    );
    let force_replaced = v2_result(
        dispatch(&production, &force_replace),
        "session.start_replace",
    );
    assert_eq!(force_replaced["session_state"], "prepared");
    let third_session = SessionId::new(force_replaced["session_id"].as_str().unwrap()).unwrap();

    let begin = request(
        141_006,
        "session.begin",
        &selector,
        Map::new(),
        "v2lif004-force-reset-begin",
        PreconditionsV1::new(
            Some(third_session.clone()),
            Some(Revision::ZERO),
            None,
            None,
            None,
            None,
        )
        .unwrap(),
    );
    let _ = v2_result(dispatch(&production, &begin), "session.begin");

    let force_reset = request(
        141_007,
        "session.reset",
        &selector,
        json!({
            "confirmed": true,
            "progress_summary": "The runtime test intentionally discards its isolated work."
        })
        .as_object()
        .unwrap()
        .clone(),
        "v2lif004-force-reset",
        PreconditionsV1::new(
            Some(third_session),
            Some(Revision::new(1)),
            None,
            None,
            None,
            None,
        )
        .unwrap(),
    );
    let reset = v2_result(dispatch(&production, &force_reset), "session.reset");
    assert_eq!(reset["mode"], "force");
    assert_eq!(reset["lifecycle"], "running");
    assert_eq!(reset["reset"], true);
}
