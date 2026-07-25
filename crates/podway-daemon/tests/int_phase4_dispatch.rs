use std::sync::{Arc, Mutex};

use podway_core::{AttemptId, JobId, Revision, SessionId, WorkspaceId};
use podway_daemon::{
    dispatch::{
        CatalogDispatchErrorMapperV1, DispatchErrorDetailsV1, DispatchFailureKindV1,
        DispatchFailureV1, DispatchResponseMetadataV1, DispatcherControlServiceV1,
        DispatcherJobOutputV1, DispatcherNextRequestV1, DispatcherPreviewServiceV1,
        DispatcherReadOutputV1, DispatcherReadServiceV1, DispatcherStatusRequestV1,
        DispatcherTerminalOutputV1, DispatcherTerminalResultV1, DispatcherWorkspaceOutputV1,
        MutationAdmissionWorkerV1, MutationDispatchOutcomeV1, MutationWaitV1,
        RequestDispatcherV1Adapter, RequestReadWaitV1, WorkspaceRuntimeV1,
    },
    server::RequestDispatcherV1,
};
use podway_protocol::{
    ClientInfoV1, CommandNameV1, IdempotencyKeyV1, JobOutputV1, JobStateV1, OperationV1,
    PreconditionsV1, RequestEnvelopeInputV1, RequestEnvelopeV1, RequestIdV1, RequestOptionsV1,
    ResponseEnvelopeV1, Rfc3339MillisV1, SliceRequestV1, WorkspaceContextV1, WorkspaceOutputV1,
    WorktreeSelectorWireV1,
};
use serde_json::{Map, Value, json};

const WORKSPACE_ID: &str = "00000000-0000-4000-8000-000000000101";
const ATTEMPT_ID: &str = "00000000-0000-4000-8000-000000000102";
const JOB_ID: &str = "00000000-0000-4000-8000-000000000103";
const SESSION_ID: &str = "00000000-0000-4000-8000-000000000104";
const GENERATED_AT: &str = "2026-07-15T12:34:56.789Z";

#[derive(Clone)]
struct FakeWorkspace {
    output: WorkspaceOutputV1,
}

#[derive(Default)]
struct RuntimeState {
    existing_selectors: Vec<String>,
    bootstrap_selectors: Vec<String>,
    existing_failure: Option<DispatchFailureV1>,
}

#[derive(Clone)]
struct FakeRuntime {
    state: Arc<Mutex<RuntimeState>>,
    workspace: FakeWorkspace,
}

impl FakeRuntime {
    fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(RuntimeState::default())),
            workspace: FakeWorkspace {
                output: WorkspaceOutputV1::new(
                    WorkspaceId::new(WORKSPACE_ID).unwrap(),
                    "/safe/worktree",
                    41,
                )
                .unwrap(),
            },
        }
    }

    fn fail_existing(&self, failure: DispatchFailureV1) {
        self.state.lock().unwrap().existing_failure = Some(failure);
    }
}

impl WorkspaceRuntimeV1 for FakeRuntime {
    type Workspace = FakeWorkspace;

    fn resolve_existing(
        &self,
        selector: &WorktreeSelectorWireV1,
    ) -> Result<Self::Workspace, DispatchFailureV1> {
        let mut state = self.state.lock().unwrap();
        state.existing_selectors.push(selector.display().to_owned());
        if let Some(failure) = state.existing_failure.clone() {
            return Err(failure);
        }
        Ok(self.workspace.clone())
    }
    fn resolve_existing_readonly(
        &self,
        selector: &WorktreeSelectorWireV1,
    ) -> Result<Self::Workspace, DispatchFailureV1> {
        self.resolve_existing(selector)
    }

    fn resolve_bootstrap(
        &self,
        selector: &WorktreeSelectorWireV1,
    ) -> Result<Self::Workspace, DispatchFailureV1> {
        self.state
            .lock()
            .unwrap()
            .bootstrap_selectors
            .push(selector.display().to_owned());
        Ok(self.workspace.clone())
    }

    fn workspace_output(&self, workspace: &Self::Workspace) -> WorkspaceOutputV1 {
        workspace.output.clone()
    }

    fn doctor(
        &self,
        _selector: &WorktreeSelectorWireV1,
        _deep: bool,
    ) -> Result<DispatcherWorkspaceOutputV1, DispatchFailureV1> {
        Ok(DispatcherWorkspaceOutputV1::new(
            self.workspace.output.clone(),
            Map::new(),
            Vec::new(),
        ))
    }

    fn show(
        &self,
        _selector: &WorktreeSelectorWireV1,
    ) -> Result<DispatcherWorkspaceOutputV1, DispatchFailureV1> {
        Ok(DispatcherWorkspaceOutputV1::new(
            self.workspace.output.clone(),
            Map::new(),
            Vec::new(),
        ))
    }

    fn repair(
        &self,
        _selector: &WorktreeSelectorWireV1,
    ) -> Result<DispatcherWorkspaceOutputV1, DispatchFailureV1> {
        Ok(DispatcherWorkspaceOutputV1::new(
            self.workspace.output.clone(),
            Map::new(),
            Vec::new(),
        ))
    }
}

#[derive(Default)]
struct ReadState {
    status_waits: Vec<RequestReadWaitV1>,
    status_verbose: Vec<bool>,
    next_waits: Vec<RequestReadWaitV1>,
    status_session_ids: Vec<Option<SessionId>>,
    next_session_ids: Vec<Option<SessionId>>,
}

#[derive(Clone)]
struct FakeReads {
    state: Arc<Mutex<ReadState>>,
}

impl FakeReads {
    fn new() -> Self {
        Self {
            state: Arc::new(Mutex::new(ReadState::default())),
        }
    }
}

fn status_result() -> Map<String, Value> {
    map(json!({
        "task": {
            "title": "Task",
            "procedure": {
                "id": "phase4",
                "version": "1",
                "name": "Phase 4",
                "digest": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            }
        },
        "session": {
            "id": "00000000-0000-4000-8000-000000000104",
            "lifecycle": "running",
            "revision": 1,
            "created_at": GENERATED_AT,
            "completed_at": null,
            "cancelled_at": null
        },
        "current": null,
        "stages": [],
        "items": [],
        "blockers": [],
        "queue": {
            "pending_mutations": true,
            "queued_count": 2,
            "running_job_id": JOB_ID,
            "latest_workspace_sequence": 41
        }
    }))
}

fn next_result() -> Map<String, Value> {
    map(json!({
        "stage": null,
        "missing_required_items": [],
        "blockers": [],
        "allowed_actions": {
            "complete": false,
            "skip": false,
            "retry": false,
            "return_to": [],
            "cancel": true
        },
        "next_stage_after_completion": null,
        "suggestions": []
    }))
}
impl DispatcherReadServiceV1<FakeWorkspace> for FakeReads {
    fn status(
        &self,
        _workspace: &FakeWorkspace,
        input: DispatcherStatusRequestV1,
    ) -> Result<DispatcherReadOutputV1, DispatchFailureV1> {
        let mut state = self.state.lock().unwrap();
        state.status_waits.push(input.wait);
        state.status_verbose.push(input.verbose);
        state.status_session_ids.push(input.expected_session_id);
        Ok(DispatcherReadOutputV1::new(status_result(), Vec::new()))
    }

    fn next(
        &self,
        _workspace: &FakeWorkspace,
        input: DispatcherNextRequestV1,
    ) -> Result<DispatcherReadOutputV1, DispatchFailureV1> {
        let mut state = self.state.lock().unwrap();
        state.next_waits.push(input.wait);
        state.next_session_ids.push(input.expected_session_id);
        Ok(DispatcherReadOutputV1::new(next_result(), Vec::new()))
    }
    fn job_list(
        &self,
        _workspace: &FakeWorkspace,
        _state: Option<JobStateV1>,
    ) -> Result<DispatcherReadOutputV1, DispatchFailureV1> {
        Ok(DispatcherReadOutputV1::new(
            map(json!({"jobs": []})),
            Vec::new(),
        ))
    }

    fn job_status(
        &self,
        _workspace: &FakeWorkspace,
        _job_id: &JobId,
        _wait: RequestReadWaitV1,
    ) -> Result<DispatcherJobOutputV1, DispatchFailureV1> {
        Ok(DispatcherJobOutputV1::new(
            queued_job(),
            map(json!({"job": {"id": JOB_ID}})),
            Vec::new(),
        ))
    }
}

#[derive(Clone, Default)]
struct FakeControl;

impl FakeControl {
    fn new() -> Self {
        Self
    }
}

impl DispatcherControlServiceV1<FakeWorkspace> for FakeControl {
    fn cancel_job(
        &self,
        _workspace: &FakeWorkspace,
        _job_id: &JobId,
        _expected_state: JobStateV1,
    ) -> Result<DispatcherJobOutputV1, DispatchFailureV1> {
        Ok(DispatcherJobOutputV1::new(
            queued_job(),
            map(json!({"cancelled": true})),
            Vec::new(),
        ))
    }
}
#[derive(Clone, Default)]
struct FakePreview;

impl DispatcherPreviewServiceV1<FakeWorkspace> for FakePreview {
    fn preview(
        &self,
        _workspace: &FakeWorkspace,
        _request: &SliceRequestV1,
    ) -> Result<DispatcherReadOutputV1, DispatchFailureV1> {
        Ok(DispatcherReadOutputV1::new(Map::new(), Vec::new()))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct MutationCall {
    command: String,
    key: String,
    wait: MutationWaitV1,
}

#[derive(Default)]
struct WorkerState {
    calls: Vec<MutationCall>,
    outcome: Option<Result<MutationDispatchOutcomeV1, DispatchFailureV1>>,
}

#[derive(Clone)]
struct FakeWorker {
    state: Arc<Mutex<WorkerState>>,
}

impl FakeWorker {
    fn new(outcome: Result<MutationDispatchOutcomeV1, DispatchFailureV1>) -> Self {
        Self {
            state: Arc::new(Mutex::new(WorkerState {
                calls: Vec::new(),
                outcome: Some(outcome),
            })),
        }
    }
}

impl MutationAdmissionWorkerV1<FakeWorkspace> for FakeWorker {
    fn admit_and_wait(
        &self,
        _workspace: &FakeWorkspace,
        request: &SliceRequestV1,
        idempotency_key: &IdempotencyKeyV1,
        wait: MutationWaitV1,
    ) -> Result<MutationDispatchOutcomeV1, DispatchFailureV1> {
        let mut state = self.state.lock().unwrap();
        state.calls.push(MutationCall {
            command: request.command().command_name().to_owned(),
            key: idempotency_key.as_str().to_owned(),
            wait,
        });
        state.outcome.clone().expect("test worker has an outcome")
    }
    fn reset_all(
        &self,
        _selector: &WorktreeSelectorWireV1,
        request: &SliceRequestV1,
        idempotency_key: &IdempotencyKeyV1,
    ) -> Result<(WorkspaceOutputV1, MutationDispatchOutcomeV1), DispatchFailureV1> {
        let mut state = self.state.lock().unwrap();
        state.calls.push(MutationCall {
            command: request.command().command_name().to_owned(),
            key: idempotency_key.as_str().to_owned(),
            wait: MutationWaitV1::UntilTerminal { timeout_millis: 0 },
        });
        let output = WorkspaceOutputV1::new(
            WorkspaceId::new(WORKSPACE_ID).expect("test workspace ID must be valid"),
            "/safe/worktree",
            42,
        )
        .expect("test workspace output must be valid");
        Ok((
            output,
            state.outcome.clone().expect("test worker has an outcome")?,
        ))
    }
}

#[derive(Clone, Copy)]
struct FixedMetadata;

impl DispatchResponseMetadataV1 for FixedMetadata {
    fn generated_at(&self) -> Rfc3339MillisV1 {
        Rfc3339MillisV1::new(GENERATED_AT).unwrap()
    }
}

type Dispatcher = RequestDispatcherV1Adapter<
    FakeRuntime,
    FakeReads,
    FakeControl,
    FakePreview,
    FakeWorker,
    FixedMetadata,
    CatalogDispatchErrorMapperV1,
>;

fn dispatcher(runtime: FakeRuntime, reads: FakeReads, worker: FakeWorker) -> Dispatcher {
    RequestDispatcherV1Adapter::new(
        runtime,
        reads,
        FakeControl::new(),
        FakePreview,
        worker,
        FixedMetadata,
        CatalogDispatchErrorMapperV1,
    )
}

fn map(value: Value) -> Map<String, Value> {
    value.as_object().unwrap().clone()
}

fn selector(display: &str) -> Value {
    serde_json::to_value(
        WorktreeSelectorWireV1::new(
            b"/safe/worktree",
            display,
            Some(WorkspaceId::new(WORKSPACE_ID).unwrap()),
        )
        .unwrap(),
    )
    .unwrap()
}

fn item_preconditions() -> PreconditionsV1 {
    PreconditionsV1::new(
        Some(SESSION_ID.to_owned().try_into().unwrap()),
        None,
        Some(AttemptId::new(ATTEMPT_ID).unwrap()),
        Some(Revision::new(3)),
        None,
        None,
    )
    .unwrap()
}

fn session_preconditions() -> PreconditionsV1 {
    PreconditionsV1::new(
        Some(SESSION_ID.to_owned().try_into().unwrap()),
        Some(Revision::new(7)),
        Some(AttemptId::new(ATTEMPT_ID).unwrap()),
        None,
        None,
        None,
    )
    .unwrap()
}

fn request_and_slice(
    command: &str,
    payload: Value,
    preconditions: PreconditionsV1,
    detach: bool,
    timeout_millis: u64,
    key: u64,
) -> (RequestEnvelopeV1, SliceRequestV1) {
    let operation = match command {
        "workspace.init" | "workspace.reset_all" => OperationV1::Bootstrap,
        "workspace.repair" | "job.cancel" => OperationV1::Control,
        "workspace.doctor" | "workspace.show" | "session.status" | "session.next" | "job.list"
        | "job.status" | "job.wait" => OperationV1::Query,
        _ => OperationV1::Mutate,
    };
    let request = RequestEnvelopeV1::new(RequestEnvelopeInputV1 {
        request_id: RequestIdV1::new(format!("00000000-0000-4000-8000-{key:012}")).unwrap(),
        client: ClientInfoV1::new("dispatch-test", "1", 1).unwrap(),
        operation,
        command: CommandNameV1::new(command).unwrap(),
        workspace: Some(
            WorkspaceContextV1::new(
                "/client/diagnostic/path",
                Some(WorkspaceId::new(WORKSPACE_ID).unwrap()),
            )
            .unwrap(),
        ),
        idempotency_key: matches!(operation, OperationV1::Mutate | OperationV1::Bootstrap)
            .then(|| IdempotencyKeyV1::new(format!("key-{key}")).unwrap()),
        preconditions,
        options: RequestOptionsV1::new(detach, timeout_millis).unwrap(),
        payload: map(payload),
    })
    .unwrap();
    let slice = SliceRequestV1::from_envelope(&request).unwrap();
    (request, slice)
}

fn queued_job() -> JobOutputV1 {
    JobOutputV1::new(
        JobId::new(JOB_ID).unwrap(),
        41,
        JobStateV1::Queued,
        Rfc3339MillisV1::new(GENERATED_AT).unwrap(),
        None,
        None,
    )
    .unwrap()
}

fn terminal_job() -> JobOutputV1 {
    JobOutputV1::new(
        JobId::new(JOB_ID).unwrap(),
        41,
        JobStateV1::Succeeded,
        Rfc3339MillisV1::new(GENERATED_AT).unwrap(),
        None,
        Some(Rfc3339MillisV1::new(GENERATED_AT).unwrap()),
    )
    .unwrap()
}

fn terminal_success() -> Result<MutationDispatchOutcomeV1, DispatchFailureV1> {
    Ok(MutationDispatchOutcomeV1::Terminal {
        job: terminal_job(),
        result: DispatcherTerminalResultV1::Output(DispatcherTerminalOutputV1::new(
            None,
            map(json!({"changed": true})),
            Vec::new(),
        )),
    })
}

fn output(response: ResponseEnvelopeV1) -> podway_protocol::OutputEnvelopeV1 {
    match response {
        ResponseEnvelopeV1::Output(output) => output,
        ResponseEnvelopeV1::Error(error) => panic!("unexpected error response: {error:?}"),
    }
}

fn error(response: ResponseEnvelopeV1) -> podway_protocol::ErrorEnvelopeV1 {
    match response {
        ResponseEnvelopeV1::Error(error) => error,
        ResponseEnvelopeV1::Output(output) => panic!("unexpected output response: {output:?}"),
    }
}

#[test]
fn routes_existing_commands_through_their_g006_authorities() {
    let runtime = FakeRuntime::new();
    let reads = FakeReads::new();
    let worker = FakeWorker::new(terminal_success());
    let dispatcher = dispatcher(runtime.clone(), reads.clone(), worker.clone());
    let selector = selector("/safe/worktree");
    let cases = vec![
        (
            "workspace.init",
            json!({"selector": selector.clone()}),
            PreconditionsV1::default(),
        ),
        (
            "session.start",
            json!({"selector": selector.clone(), "preset": "sw-dev", "task_title": "Task"}),
            PreconditionsV1::default(),
        ),
        (
            "session.status",
            json!({"selector": selector.clone()}),
            PreconditionsV1::default(),
        ),
        (
            "session.next",
            json!({"selector": selector.clone()}),
            PreconditionsV1::default(),
        ),
        (
            "item.check",
            json!({"selector": selector.clone(), "item_id": "confirm"}),
            item_preconditions(),
        ),
        (
            "item.set",
            json!({"selector": selector.clone(), "item_id": "notes", "value": "value"}),
            item_preconditions(),
        ),
        (
            "item.add",
            json!({"selector": selector.clone(), "item_id": "entries", "value": "value"}),
            item_preconditions(),
        ),
        (
            "item.attach",
            json!({"selector": selector.clone(), "item_id": "artifact", "path": "proof.txt", "media_type": "text/plain"}),
            item_preconditions(),
        ),
        (
            "session.block",
            json!({"selector": selector.clone(), "reason": "waiting"}),
            session_preconditions(),
        ),
        (
            "session.unblock",
            json!({"selector": selector.clone(), "all": true}),
            session_preconditions(),
        ),
        (
            "session.retry",
            json!({"selector": selector.clone(), "reason": "retry"}),
            session_preconditions(),
        ),
        (
            "session.return",
            json!({"selector": selector.clone(), "destination_stage_id": "review", "reason": "revisit"}),
            session_preconditions(),
        ),
        (
            "session.complete",
            json!({"selector": selector.clone()}),
            session_preconditions(),
        ),
    ];

    for (index, (command, payload, preconditions)) in cases.into_iter().enumerate() {
        let (request, slice) =
            request_and_slice(command, payload, preconditions, false, 0, index as u64 + 1);
        let response = dispatcher.dispatch(&request, &slice);
        assert_eq!(
            match response {
                ResponseEnvelopeV1::Output(ref output) => output.command().as_str(),
                ResponseEnvelopeV1::Error(ref error) => error.command().as_str(),
            },
            command
        );
    }

    let worker_state = worker.state.lock().unwrap();
    let calls = &worker_state.calls;
    assert_eq!(
        calls
            .iter()
            .map(|call| call.command.as_str())
            .collect::<Vec<_>>(),
        vec![
            "workspace.init",
            "session.start",
            "item.check",
            "item.set",
            "item.add",
            "item.attach",
            "session.block",
            "session.unblock",
            "session.retry",
            "session.return",
            "session.complete",
        ]
    );
    assert_eq!(runtime.state.lock().unwrap().bootstrap_selectors.len(), 1);
    assert_eq!(reads.state.lock().unwrap().status_waits.len(), 1);
    assert_eq!(reads.state.lock().unwrap().next_waits.len(), 1);
}

#[test]
fn workspace_init_bootstraps_without_starting_a_task() {
    let runtime = FakeRuntime::new();
    let reads = FakeReads::new();
    let worker = FakeWorker::new(Ok(MutationDispatchOutcomeV1::Terminal {
        job: terminal_job(),
        result: DispatcherTerminalResultV1::Output(DispatcherTerminalOutputV1::new(
            None,
            map(json!({"initialized": true})),
            Vec::new(),
        )),
    }));
    let dispatcher = dispatcher(runtime.clone(), reads, worker.clone());
    let (request, slice) = request_and_slice(
        "workspace.init",
        json!({"selector": selector("/safe/worktree")}),
        PreconditionsV1::default(),
        false,
        1,
        30,
    );

    let response = output(dispatcher.dispatch(&request, &slice));
    assert!(response.session().is_none());
    assert_eq!(
        response.result().get("initialized"),
        Some(&Value::Bool(true))
    );
    assert_eq!(runtime.state.lock().unwrap().bootstrap_selectors.len(), 1);
    assert!(runtime.state.lock().unwrap().existing_selectors.is_empty());
    assert_eq!(
        worker.state.lock().unwrap().calls[0].command,
        "workspace.init"
    );
}

#[test]
fn queries_preserve_pending_fields_and_use_the_request_wait() {
    let runtime = FakeRuntime::new();
    let reads = FakeReads::new();
    let worker = FakeWorker::new(terminal_success());
    let dispatcher = dispatcher(runtime, reads.clone(), worker);
    let (request, slice) = request_and_slice(
        "session.status",
        json!({"selector": selector("/safe/worktree"), "wait_for_idle": true, "verbose": true}),
        PreconditionsV1::new(
            Some(SessionId::new(SESSION_ID).unwrap()),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap(),
        false,
        73,
        31,
    );

    let response = output(dispatcher.dispatch(&request, &slice));
    assert_eq!(
        response.result()["queue"]["pending_mutations"],
        Value::Bool(true)
    );
    assert_eq!(response.result()["queue"]["queued_count"], Value::from(2));
    assert_eq!(
        reads.state.lock().unwrap().status_waits,
        vec![RequestReadWaitV1::IdleUntil { timeout_millis: 73 }]
    );
    assert_eq!(reads.state.lock().unwrap().status_verbose, vec![true]);
    assert_eq!(
        reads.state.lock().unwrap().status_session_ids,
        vec![Some(SessionId::new(SESSION_ID).unwrap())]
    );
}

#[test]
fn detached_mutation_returns_a_durable_receipt_without_waiting() {
    let runtime = FakeRuntime::new();
    let reads = FakeReads::new();
    let worker = FakeWorker::new(Ok(MutationDispatchOutcomeV1::Detached {
        job: queued_job(),
    }));
    let dispatcher = dispatcher(runtime, reads, worker.clone());
    let (request, slice) = request_and_slice(
        "session.start",
        json!({"selector": selector("/safe/worktree"), "preset": "sw-dev", "task_title": "Task"}),
        PreconditionsV1::default(),
        true,
        90,
        32,
    );

    let response = output(dispatcher.dispatch(&request, &slice));
    assert_eq!(response.job().unwrap().id().as_str(), JOB_ID);
    assert_eq!(response.result()["admitted"], Value::Bool(true));
    assert_eq!(response.result()["detached"], Value::Bool(true));
    assert_eq!(
        worker.state.lock().unwrap().calls[0].wait,
        MutationWaitV1::Detached
    );
}

#[test]
fn synchronous_mutation_returns_the_immutable_terminal_response() {
    let runtime = FakeRuntime::new();
    let reads = FakeReads::new();
    let worker = FakeWorker::new(terminal_success());
    let dispatcher = dispatcher(runtime, reads, worker.clone());
    let (request, slice) = request_and_slice(
        "session.complete",
        json!({"selector": selector("/safe/worktree")}),
        session_preconditions(),
        false,
        91,
        33,
    );

    let response = output(dispatcher.dispatch(&request, &slice));
    assert_eq!(response.job().unwrap().state(), JobStateV1::Succeeded);
    assert_eq!(response.result()["changed"], Value::Bool(true));
    assert_eq!(
        worker.state.lock().unwrap().calls[0].wait,
        MutationWaitV1::UntilTerminal { timeout_millis: 91 }
    );
}

#[test]
fn synchronous_timeout_reports_the_admitted_job_without_cancellation() {
    let runtime = FakeRuntime::new();
    let reads = FakeReads::new();
    let worker = FakeWorker::new(Ok(MutationDispatchOutcomeV1::TimedOut {
        job: queued_job(),
    }));
    let dispatcher = dispatcher(runtime, reads, worker.clone());
    let (request, slice) = request_and_slice(
        "session.complete",
        json!({"selector": selector("/safe/worktree")}),
        session_preconditions(),
        false,
        92,
        34,
    );

    let response = error(dispatcher.dispatch(&request, &slice));
    assert_eq!(response.code().as_str(), "JOB_WAIT_TIMEOUT");
    assert!(response.retryable());
    assert_eq!(response.exit_code().get(), 4);
    assert_eq!(
        response.details()["job_id"],
        Value::String(JOB_ID.to_owned())
    );
    assert_eq!(response.details()["job_sequence"], Value::from(41));
    assert_eq!(worker.state.lock().unwrap().calls.len(), 1);
}

#[test]
fn idempotent_replay_preserves_the_original_job_identity() {
    let runtime = FakeRuntime::new();
    let reads = FakeReads::new();
    let worker = FakeWorker::new(Ok(MutationDispatchOutcomeV1::Detached {
        job: queued_job(),
    }));
    let dispatcher = dispatcher(runtime, reads, worker.clone());
    let (request, slice) = request_and_slice(
        "session.start",
        json!({"selector": selector("/safe/worktree"), "preset": "sw-dev", "task_title": "Task"}),
        PreconditionsV1::default(),
        true,
        0,
        35,
    );

    let first = output(dispatcher.dispatch(&request, &slice));
    let second = output(dispatcher.dispatch(&request, &slice));
    assert_eq!(first.job().unwrap().id(), second.job().unwrap().id());
    assert_eq!(
        worker
            .state
            .lock()
            .unwrap()
            .calls
            .iter()
            .map(|call| call.key.as_str())
            .collect::<Vec<_>>(),
        vec!["key-35", "key-35"]
    );
}

#[test]
fn stale_terminal_conditions_are_retryable_and_include_only_safe_revisions() {
    let runtime = FakeRuntime::new();
    let reads = FakeReads::new();
    let worker = FakeWorker::new(Ok(MutationDispatchOutcomeV1::Terminal {
        job: terminal_job(),
        result: DispatcherTerminalResultV1::Error(
            DispatchFailureV1::new(DispatchFailureKindV1::SessionRevisionConflict).with_details(
                DispatchErrorDetailsV1::default()
                    .with_expected_revision(Revision::new(7))
                    .with_current_revision(Revision::new(8)),
            ),
        ),
    }));
    let dispatcher = dispatcher(runtime, reads, worker);
    let (request, slice) = request_and_slice(
        "session.complete",
        json!({"selector": selector("/safe/worktree")}),
        session_preconditions(),
        false,
        100,
        36,
    );

    let response = error(dispatcher.dispatch(&request, &slice));
    assert_eq!(response.code().as_str(), "SESSION_REVISION_CONFLICT");
    assert!(response.retryable());
    assert_eq!(response.exit_code().get(), 4);
    assert_eq!(response.details()["expected_revision"], Value::from(7));
    assert_eq!(response.details()["current_revision"], Value::from(8));
    assert_eq!(
        response.details()["job_id"],
        Value::String(JOB_ID.to_owned())
    );
}

#[test]
fn unsupported_and_malformed_requests_are_rejected_before_dispatch() {
    let unsupported = RequestEnvelopeV1::new(RequestEnvelopeInputV1 {
        request_id: RequestIdV1::new("00000000-0000-4000-8000-000000000037").unwrap(),
        client: ClientInfoV1::new("dispatch-test", "1", 1).unwrap(),
        operation: OperationV1::Bootstrap,
        command: CommandNameV1::new("workspace.initialize").unwrap(),
        workspace: Some(WorkspaceContextV1::new("/safe/worktree", None).unwrap()),
        idempotency_key: Some(IdempotencyKeyV1::new("unsupported").unwrap()),
        preconditions: PreconditionsV1::default(),
        options: RequestOptionsV1::new(false, 0).unwrap(),
        payload: map(json!({"selector": selector("/safe/worktree")})),
    })
    .unwrap();
    assert!(SliceRequestV1::from_envelope(&unsupported).is_err());

    let malformed = RequestEnvelopeV1::new(RequestEnvelopeInputV1 {
        request_id: RequestIdV1::new("00000000-0000-4000-8000-000000000038").unwrap(),
        client: ClientInfoV1::new("dispatch-test", "1", 1).unwrap(),
        operation: OperationV1::Mutate,
        command: CommandNameV1::new("session.status").unwrap(),
        workspace: Some(WorkspaceContextV1::new("/safe/worktree", None).unwrap()),
        idempotency_key: Some(IdempotencyKeyV1::new("malformed").unwrap()),
        preconditions: PreconditionsV1::default(),
        options: RequestOptionsV1::new(false, 0).unwrap(),
        payload: map(json!({"selector": selector("/safe/worktree")})),
    })
    .unwrap();
    assert!(SliceRequestV1::from_envelope(&malformed).is_err());
}

#[test]
fn moved_aliases_route_to_the_same_durable_workspace_identity() {
    let runtime = FakeRuntime::new();
    let reads = FakeReads::new();
    let worker = FakeWorker::new(terminal_success());
    let dispatcher = dispatcher(runtime.clone(), reads, worker);
    let (first_request, first_slice) = request_and_slice(
        "session.status",
        json!({"selector": selector("/old/diagnostic/alias")}),
        PreconditionsV1::default(),
        false,
        0,
        39,
    );
    let (second_request, second_slice) = request_and_slice(
        "session.status",
        json!({"selector": selector("/new/diagnostic/alias")}),
        PreconditionsV1::default(),
        false,
        0,
        40,
    );

    let first = output(dispatcher.dispatch(&first_request, &first_slice));
    let second = output(dispatcher.dispatch(&second_request, &second_slice));
    assert_eq!(
        first.workspace().unwrap().uuid(),
        second.workspace().unwrap().uuid()
    );
    assert_eq!(
        runtime.state.lock().unwrap().existing_selectors,
        vec!["/old/diagnostic/alias", "/new/diagnostic/alias"]
    );
}

#[test]
fn errors_preserve_request_correlation_and_never_reflect_runtime_diagnostics() {
    let runtime = FakeRuntime::new();
    runtime.fail_existing(DispatchFailureV1::new(
        DispatchFailureKindV1::WorkspaceStateUnreadable,
    ));
    let reads = FakeReads::new();
    let worker = FakeWorker::new(terminal_success());
    let dispatcher = dispatcher(runtime, reads, worker);
    let (request, slice) = request_and_slice(
        "session.next",
        json!({"selector": selector("/client/secret.sqlite")}),
        PreconditionsV1::default(),
        false,
        0,
        41,
    );

    let response = error(dispatcher.dispatch(&request, &slice));
    assert_eq!(response.request_id(), request.request_id());
    assert_eq!(response.command(), request.command());
    assert_eq!(response.code().as_str(), "WORKSPACE_STATE_UNREADABLE");
    assert_eq!(response.exit_code().get(), 5);
    assert!(!response.message().contains("secret.sqlite"));
    assert!(response.details().is_empty());
}

#[test]
fn identity_conflicts_use_closed_details_before_and_after_admission() {
    let expected_workspace = WorkspaceId::new("00000000-0000-4000-8000-000000000105").unwrap();
    let actual_workspace = WorkspaceId::new(WORKSPACE_ID).unwrap();
    let runtime = FakeRuntime::new();
    runtime.fail_existing(
        DispatchFailureV1::new(DispatchFailureKindV1::WorkspaceUuidMismatch).with_details(
            DispatchErrorDetailsV1::default()
                .with_workspace_uuid_mismatch(expected_workspace, actual_workspace),
        ),
    );
    let preadmission_dispatcher = dispatcher(
        runtime,
        FakeReads::new(),
        FakeWorker::new(terminal_success()),
    );
    let (request, slice) = request_and_slice(
        "session.status",
        json!({"selector": selector("/safe/worktree")}),
        PreconditionsV1::default(),
        false,
        0,
        42,
    );
    let response = error(preadmission_dispatcher.dispatch(&request, &slice));
    assert_eq!(response.code().as_str(), "WORKSPACE_UUID_MISMATCH");
    assert_eq!(response.exit_code().get(), 4);
    assert!(!response.retryable());
    assert_eq!(
        response.details(),
        &Map::from_iter([
            (
                "schema".to_owned(),
                json!("podway.workspace-uuid-mismatch-details/v1"),
            ),
            (
                "expected_workspace_uuid".to_owned(),
                json!("00000000-0000-4000-8000-000000000105"),
            ),
            ("actual_workspace_uuid".to_owned(), json!(WORKSPACE_ID)),
            ("admission".to_owned(), json!({"admitted": false})),
        ])
    );

    let runtime = FakeRuntime::new();
    runtime.fail_existing(DispatchFailureV1::new(
        DispatchFailureKindV1::WorkspaceUuidMismatch,
    ));
    let malformed_dispatcher = dispatcher(
        runtime,
        FakeReads::new(),
        FakeWorker::new(terminal_success()),
    );
    let response = error(malformed_dispatcher.dispatch(&request, &slice));
    assert_eq!(response.code().as_str(), "INTERNAL_ERROR");
    assert_eq!(response.exit_code().get(), 6);
    assert!(response.details().is_empty());

    let expected_session = SessionId::new(SESSION_ID).unwrap();
    let actual_session = SessionId::new("00000000-0000-4000-8000-000000000106").unwrap();
    let worker = FakeWorker::new(Ok(MutationDispatchOutcomeV1::Terminal {
        job: terminal_job(),
        result: DispatcherTerminalResultV1::Error(
            DispatchFailureV1::new(DispatchFailureKindV1::SessionIdMismatch).with_details(
                DispatchErrorDetailsV1::default()
                    .with_session_id_mismatch(expected_session, Some(actual_session)),
            ),
        ),
    }));
    let dispatcher = dispatcher(FakeRuntime::new(), FakeReads::new(), worker);
    let (request, slice) = request_and_slice(
        "session.complete",
        json!({"selector": selector("/safe/worktree")}),
        session_preconditions(),
        false,
        100,
        43,
    );
    let response = error(dispatcher.dispatch(&request, &slice));
    assert_eq!(response.code().as_str(), "SESSION_ID_MISMATCH");
    assert_eq!(response.exit_code().get(), 4);
    assert!(!response.retryable());
    assert_eq!(
        response.details(),
        &Map::from_iter([
            (
                "schema".to_owned(),
                json!("podway.session-id-mismatch-details/v1"),
            ),
            ("expected_session_id".to_owned(), json!(SESSION_ID)),
            (
                "actual_session_id".to_owned(),
                json!("00000000-0000-4000-8000-000000000106"),
            ),
            (
                "admission".to_owned(),
                json!({
                    "admitted": true,
                    "job_id": JOB_ID,
                    "workspace_sequence": 41
                }),
            ),
        ])
    );
}
