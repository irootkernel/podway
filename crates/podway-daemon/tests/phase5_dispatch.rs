use std::sync::{Arc, Mutex};

use podway_core::{AttemptId, JobId, Revision, SessionId, WorkspaceId};
use podway_daemon::{
    dispatch::{
        CatalogDispatchErrorMapperV1, DispatchErrorDetailsV1, DispatchFailureKindV1,
        DispatchFailureV1, DispatchResponseMetadataV1, DispatcherControlServiceV1,
        DispatcherJobOutputV1, DispatcherPreviewServiceV1, DispatcherReadOutputV1,
        DispatcherReadServiceV1, DispatcherStatusRequestV1, DispatcherTerminalOutputV1,
        DispatcherTerminalResultV1, DispatcherWorkspaceOutputV1, MutationAdmissionWorkerV1,
        MutationDispatchOutcomeV1, MutationWaitV1, RequestDispatcherV1Adapter, RequestReadWaitV1,
        WorkspaceRuntimeV1,
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

const WORKSPACE_ID: &str = "00000000-0000-4000-8000-000000000501";
const SESSION_ID: &str = "00000000-0000-4000-8000-000000000502";
const ATTEMPT_ID: &str = "00000000-0000-4000-8000-000000000503";
const JOB_ID: &str = "00000000-0000-4000-8000-000000000504";
const GENERATED_AT: &str = "2026-07-16T12:34:56.789Z";

#[derive(Clone)]
struct Workspace {
    output: WorkspaceOutputV1,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MutationAuthority {
    Admission,
    ResetAllMaintenance,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct MutationCall {
    authority: MutationAuthority,
    command: String,
    idempotency_key: String,
    wait: Option<MutationWaitV1>,
}

#[derive(Default)]
struct Calls {
    runtime: Vec<&'static str>,
    reads: Vec<(&'static str, Option<RequestReadWaitV1>)>,
    status_verbose: Vec<bool>,
    invalid_read_route: Option<&'static str>,
    controls: Vec<JobStateV1>,
    previews: usize,
    mutations: Vec<MutationCall>,
}

#[derive(Clone)]
struct Runtime {
    workspace: Workspace,
    calls: Arc<Mutex<Calls>>,
}

impl WorkspaceRuntimeV1 for Runtime {
    type Workspace = Workspace;

    fn resolve_existing(
        &self,
        _selector: &WorktreeSelectorWireV1,
    ) -> Result<Self::Workspace, DispatchFailureV1> {
        self.calls.lock().unwrap().runtime.push("existing");
        Ok(self.workspace.clone())
    }
    fn resolve_existing_readonly(
        &self,
        _selector: &WorktreeSelectorWireV1,
    ) -> Result<Self::Workspace, DispatchFailureV1> {
        self.calls.lock().unwrap().runtime.push("readonly");
        Ok(self.workspace.clone())
    }

    fn resolve_bootstrap(
        &self,
        _selector: &WorktreeSelectorWireV1,
    ) -> Result<Self::Workspace, DispatchFailureV1> {
        self.calls.lock().unwrap().runtime.push("bootstrap");
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
        self.calls.lock().unwrap().runtime.push("doctor");
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
        self.calls.lock().unwrap().runtime.push("show");
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
        self.calls.lock().unwrap().runtime.push("repair");
        Ok(DispatcherWorkspaceOutputV1::new(
            self.workspace.output.clone(),
            Map::from_iter([("repaired".to_owned(), Value::Bool(true))]),
            Vec::new(),
        ))
    }
}

#[derive(Clone)]
struct Reads(Arc<Mutex<Calls>>);

fn status_result() -> Map<String, Value> {
    json!({
        "task": {
            "title": "Task",
            "procedure": {
                "id": "phase5",
                "version": "1",
                "name": "Phase 5",
                "digest": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
            }
        },
        "session": {
            "id": SESSION_ID,
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
            "pending_mutations": false,
            "queued_count": 0,
            "running_job_id": null,
            "latest_workspace_sequence": 1
        }
    })
    .as_object()
    .unwrap()
    .clone()
}

fn next_result() -> Map<String, Value> {
    json!({
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
    })
    .as_object()
    .unwrap()
    .clone()
}
impl DispatcherReadServiceV1<Workspace> for Reads {
    fn status(
        &self,
        _workspace: &Workspace,
        input: DispatcherStatusRequestV1,
    ) -> Result<DispatcherReadOutputV1, DispatchFailureV1> {
        let invalid = {
            let mut calls = self.0.lock().unwrap();
            calls.reads.push(("status", Some(input.wait)));
            calls.status_verbose.push(input.verbose);
            calls.invalid_read_route == Some("status")
        };
        Ok(DispatcherReadOutputV1::new(
            if invalid { Map::new() } else { status_result() },
            Vec::new(),
        ))
    }

    fn next(
        &self,
        _workspace: &Workspace,
        wait: RequestReadWaitV1,
    ) -> Result<DispatcherReadOutputV1, DispatchFailureV1> {
        let invalid = {
            let mut calls = self.0.lock().unwrap();
            calls.reads.push(("next", Some(wait)));
            calls.invalid_read_route == Some("next")
        };
        Ok(DispatcherReadOutputV1::new(
            if invalid { Map::new() } else { next_result() },
            Vec::new(),
        ))
    }

    fn job_list(
        &self,
        _workspace: &Workspace,
        _state: Option<JobStateV1>,
    ) -> Result<DispatcherReadOutputV1, DispatchFailureV1> {
        self.0.lock().unwrap().reads.push(("job.list", None));
        Ok(DispatcherReadOutputV1::new(Map::new(), Vec::new()))
    }

    fn job_status(
        &self,
        _workspace: &Workspace,
        _job_id: &JobId,
        wait: RequestReadWaitV1,
    ) -> Result<DispatcherJobOutputV1, DispatchFailureV1> {
        self.0.lock().unwrap().reads.push(("job", Some(wait)));
        Ok(DispatcherJobOutputV1::new(
            queued_job(),
            Map::new(),
            Vec::new(),
        ))
    }
}

#[derive(Clone)]
struct Controls(Arc<Mutex<Calls>>);

impl DispatcherControlServiceV1<Workspace> for Controls {
    fn cancel_job(
        &self,
        _workspace: &Workspace,
        _job_id: &JobId,
        expected_state: JobStateV1,
    ) -> Result<DispatcherJobOutputV1, DispatchFailureV1> {
        self.0.lock().unwrap().controls.push(expected_state);
        if expected_state != JobStateV1::Queued {
            return Err(DispatchFailureV1::new(
                DispatchFailureKindV1::JobNotCancellable,
            ));
        }
        Ok(DispatcherJobOutputV1::new(
            queued_job(),
            Map::new(),
            Vec::new(),
        ))
    }
}

#[derive(Clone)]
struct Previews(Arc<Mutex<Calls>>);

impl DispatcherPreviewServiceV1<Workspace> for Previews {
    fn preview(
        &self,
        _workspace: &Workspace,
        _request: &SliceRequestV1,
    ) -> Result<DispatcherReadOutputV1, DispatchFailureV1> {
        self.0.lock().unwrap().previews += 1;
        Ok(DispatcherReadOutputV1::new(
            Map::from_iter([("preview".to_owned(), Value::Bool(true))]),
            Vec::new(),
        ))
    }
}

#[derive(Clone)]
struct Mutations {
    calls: Arc<Mutex<Calls>>,
    output: WorkspaceOutputV1,
    outcome: Result<MutationDispatchOutcomeV1, DispatchFailureV1>,
}

impl MutationAdmissionWorkerV1<Workspace> for Mutations {
    fn admit_and_wait(
        &self,
        _workspace: &Workspace,
        request: &SliceRequestV1,
        idempotency_key: &IdempotencyKeyV1,
        wait: MutationWaitV1,
    ) -> Result<MutationDispatchOutcomeV1, DispatchFailureV1> {
        self.calls.lock().unwrap().mutations.push(MutationCall {
            authority: MutationAuthority::Admission,
            command: request.command().command_name().to_owned(),
            idempotency_key: idempotency_key.as_str().to_owned(),
            wait: Some(wait),
        });
        self.outcome.clone()
    }

    fn reset_all(
        &self,
        _selector: &WorktreeSelectorWireV1,
        request: &SliceRequestV1,
        idempotency_key: &IdempotencyKeyV1,
    ) -> Result<(WorkspaceOutputV1, MutationDispatchOutcomeV1), DispatchFailureV1> {
        self.calls.lock().unwrap().mutations.push(MutationCall {
            authority: MutationAuthority::ResetAllMaintenance,
            command: request.command().command_name().to_owned(),
            idempotency_key: idempotency_key.as_str().to_owned(),
            wait: None,
        });
        Ok((self.output.clone(), self.outcome.clone()?))
    }
}

#[derive(Clone, Copy)]
struct Metadata;

impl DispatchResponseMetadataV1 for Metadata {
    fn generated_at(&self) -> Rfc3339MillisV1 {
        Rfc3339MillisV1::new(GENERATED_AT).unwrap()
    }
}

type Dispatcher = RequestDispatcherV1Adapter<
    Runtime,
    Reads,
    Controls,
    Previews,
    Mutations,
    Metadata,
    CatalogDispatchErrorMapperV1,
>;

fn dispatcher(calls: Arc<Mutex<Calls>>) -> Dispatcher {
    dispatcher_with_outcome(calls, terminal_success())
}

fn dispatcher_with_outcome(
    calls: Arc<Mutex<Calls>>,
    outcome: Result<MutationDispatchOutcomeV1, DispatchFailureV1>,
) -> Dispatcher {
    let workspace = Workspace {
        output: WorkspaceOutputV1::new(
            WorkspaceId::new(WORKSPACE_ID).unwrap(),
            "/safe/worktree",
            1,
        )
        .unwrap(),
    };
    RequestDispatcherV1Adapter::new(
        Runtime {
            workspace: workspace.clone(),
            calls: Arc::clone(&calls),
        },
        Reads(Arc::clone(&calls)),
        Controls(Arc::clone(&calls)),
        Previews(Arc::clone(&calls)),
        Mutations {
            calls,
            output: workspace.output,
            outcome,
        },
        Metadata,
        CatalogDispatchErrorMapperV1,
    )
}

fn selector() -> Value {
    serde_json::to_value(
        WorktreeSelectorWireV1::new(
            b"/safe/worktree",
            "/diagnostic/alias",
            Some(WorkspaceId::new(WORKSPACE_ID).unwrap()),
        )
        .unwrap(),
    )
    .unwrap()
}

fn item_preconditions() -> PreconditionsV1 {
    PreconditionsV1::new(
        None,
        None,
        Some(AttemptId::new(ATTEMPT_ID).unwrap()),
        Some(Revision::new(1)),
        None,
        None,
    )
    .unwrap()
}

fn session_preconditions() -> PreconditionsV1 {
    PreconditionsV1::new(
        None,
        Some(Revision::new(1)),
        Some(AttemptId::new(ATTEMPT_ID).unwrap()),
        None,
        None,
        None,
    )
    .unwrap()
}

fn session_identity_preconditions() -> PreconditionsV1 {
    PreconditionsV1::new(
        Some(SessionId::new(SESSION_ID).unwrap()),
        Some(Revision::new(1)),
        None,
        None,
        None,
        None,
    )
    .unwrap()
}

fn session_revision_preconditions() -> PreconditionsV1 {
    PreconditionsV1::new(None, Some(Revision::new(1)), None, None, None, None).unwrap()
}

fn job_preconditions(state: JobStateV1) -> PreconditionsV1 {
    PreconditionsV1::new(None, None, None, None, None, Some(state)).unwrap()
}

fn operation(command: &str) -> OperationV1 {
    match command {
        "workspace.init" | "workspace.reset_all" => OperationV1::Bootstrap,
        "workspace.repair" | "job.cancel" => OperationV1::Control,
        "workspace.doctor" | "workspace.show" | "session.status" | "session.next" | "job.list"
        | "job.status" | "job.wait" => OperationV1::Query,
        _ => OperationV1::Mutate,
    }
}

fn request(
    sequence: u64,
    command: &str,
    payload: Value,
    preconditions: PreconditionsV1,
) -> (RequestEnvelopeV1, SliceRequestV1) {
    request_with_options(sequence, command, payload, preconditions, false, 30_000)
}

fn request_with_options(
    sequence: u64,
    command: &str,
    payload: Value,
    preconditions: PreconditionsV1,
    detach: bool,
    wait_timeout_ms: u64,
) -> (RequestEnvelopeV1, SliceRequestV1) {
    let operation = if matches!(payload.get("dry_run"), Some(Value::Bool(true))) {
        OperationV1::Query
    } else {
        operation(command)
    };
    let envelope = RequestEnvelopeV1::new(RequestEnvelopeInputV1 {
        request_id: RequestIdV1::new(format!("00000000-0000-4000-8000-{sequence:012}")).unwrap(),
        client: ClientInfoV1::new("phase5-dispatch", "1", 1).unwrap(),
        operation,
        command: CommandNameV1::new(command).unwrap(),
        workspace: Some(WorkspaceContextV1::new("/client/path", None).unwrap()),
        idempotency_key: matches!(operation, OperationV1::Mutate | OperationV1::Bootstrap)
            .then(|| IdempotencyKeyV1::new(format!("phase5-{sequence}")).unwrap()),
        preconditions,
        options: RequestOptionsV1::new(detach, wait_timeout_ms).unwrap(),
        payload: payload.as_object().unwrap().clone(),
    })
    .unwrap();
    let slice = SliceRequestV1::from_envelope(&envelope).unwrap();
    (envelope, slice)
}

fn queued_job() -> JobOutputV1 {
    JobOutputV1::new(
        JobId::new(JOB_ID).unwrap(),
        1,
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
        1,
        JobStateV1::Succeeded,
        Rfc3339MillisV1::new(GENERATED_AT).unwrap(),
        None,
        Some(Rfc3339MillisV1::new(GENERATED_AT).unwrap()),
    )
    .unwrap()
}
fn failed_job() -> JobOutputV1 {
    JobOutputV1::new(
        JobId::new(JOB_ID).unwrap(),
        1,
        JobStateV1::Failed,
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
            Map::from_iter([("changed".to_owned(), Value::Bool(true))]),
            Vec::new(),
        )),
    })
}

#[test]
fn every_g006_route_reaches_its_sole_authority() {
    let calls = Arc::new(Mutex::new(Calls::default()));
    let dispatcher = dispatcher(Arc::clone(&calls));
    let selector = selector();
    let cases = vec![
        (
            "workspace.init",
            json!({"selector": selector.clone(), "repair": false}),
            PreconditionsV1::default(),
        ),
        (
            "workspace.doctor",
            json!({"selector": selector.clone(), "deep": true}),
            PreconditionsV1::default(),
        ),
        (
            "workspace.show",
            json!({"selector": selector.clone()}),
            PreconditionsV1::default(),
        ),
        (
            "workspace.repair",
            json!({"selector": selector.clone()}),
            PreconditionsV1::default(),
        ),
        (
            "session.start",
            json!({"selector": selector.clone(), "preset": "sw-dev", "task_title": "Task"}),
            PreconditionsV1::default(),
        ),
        (
            "session.start_replace",
            json!({"selector": selector.clone(), "preset": "sw-dev", "task_title": "Task", "confirmed": true}),
            session_identity_preconditions(),
        ),
        (
            "session.status",
            json!({"selector": selector.clone(), "wait_for_idle": true, "verbose": true}),
            PreconditionsV1::default(),
        ),
        (
            "session.next",
            json!({"selector": selector.clone(), "after_job_id": JOB_ID}),
            PreconditionsV1::default(),
        ),
        (
            "session.complete",
            json!({"selector": selector.clone()}),
            session_preconditions(),
        ),
        (
            "session.skip",
            json!({"selector": selector.clone(), "reason": "skip"}),
            session_preconditions(),
        ),
        (
            "session.retry",
            json!({"selector": selector.clone(), "reason": "retry"}),
            session_preconditions(),
        ),
        (
            "session.return",
            json!({"selector": selector.clone(), "destination_stage_id": "review", "reason": "return"}),
            session_preconditions(),
        ),
        (
            "session.block",
            json!({"selector": selector.clone(), "reason": "blocked"}),
            session_preconditions(),
        ),
        (
            "session.unblock",
            json!({"selector": selector.clone(), "all": true}),
            session_preconditions(),
        ),
        (
            "session.cancel",
            json!({"selector": selector.clone(), "reason": "cancel"}),
            session_preconditions(),
        ),
        (
            "session.reopen",
            json!({"selector": selector.clone(), "destination_stage_id": "review", "reason": "reopen"}),
            session_revision_preconditions(),
        ),
        (
            "session.reset",
            json!({"selector": selector.clone(), "confirmed": true}),
            session_identity_preconditions(),
        ),
        (
            "workspace.reset_all",
            json!({"selector": selector.clone(), "confirmed": true, "expected_workspace_uuid": WORKSPACE_ID}),
            PreconditionsV1::default(),
        ),
        (
            "item.check",
            json!({"selector": selector.clone(), "item_id": "confirm"}),
            item_preconditions(),
        ),
        (
            "item.uncheck",
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
            "item.remove",
            json!({"selector": selector.clone(), "item_id": "entries", "value": "value", "ignore_missing": false}),
            item_preconditions(),
        ),
        (
            "item.attach",
            json!({"selector": selector.clone(), "item_id": "artifact", "path": "proof.txt", "media_type": "text/plain"}),
            item_preconditions(),
        ),
        (
            "item.clear",
            json!({"selector": selector.clone(), "item_id": "artifact"}),
            item_preconditions(),
        ),
        (
            "job.list",
            json!({"selector": selector.clone(), "state": "queued"}),
            PreconditionsV1::default(),
        ),
        (
            "job.status",
            json!({"selector": selector.clone(), "job_id": JOB_ID}),
            PreconditionsV1::default(),
        ),
        (
            "job.wait",
            json!({"selector": selector.clone(), "job_id": JOB_ID}),
            PreconditionsV1::default(),
        ),
        (
            "job.cancel",
            json!({"selector": selector.clone(), "job_id": JOB_ID}),
            job_preconditions(JobStateV1::Queued),
        ),
    ];

    for (index, (command, payload, preconditions)) in cases.into_iter().enumerate() {
        let (request, slice) = request(index as u64 + 1, command, payload, preconditions);
        let response = dispatcher.dispatch(&request, &slice);
        match response {
            ResponseEnvelopeV1::Output(output) => {
                assert_eq!(output.request_id(), request.request_id(), "{command}");
                assert_eq!(output.command(), request.command(), "{command}");
            }
            ResponseEnvelopeV1::Error(error) => panic!("unexpected {command} error: {error:?}"),
        }
    }
    let calls = calls.lock().unwrap();
    assert_eq!(
        calls
            .runtime
            .iter()
            .filter(|route| **route == "doctor")
            .count(),
        1
    );
    assert_eq!(
        calls
            .runtime
            .iter()
            .filter(|route| **route == "show")
            .count(),
        1
    );
    assert_eq!(
        calls
            .runtime
            .iter()
            .filter(|route| **route == "repair")
            .count(),
        1
    );
    assert_eq!(calls.controls, vec![JobStateV1::Queued]);
    assert_eq!(
        calls.reads,
        vec![
            (
                "status",
                Some(RequestReadWaitV1::IdleUntil {
                    timeout_millis: 30_000
                })
            ),
            (
                "next",
                Some(RequestReadWaitV1::AfterJobUntil {
                    job_id: JobId::new(JOB_ID).unwrap(),
                    timeout_millis: 30_000
                })
            ),
            ("job.list", None),
            ("job", Some(RequestReadWaitV1::Immediate)),
            (
                "job",
                Some(RequestReadWaitV1::AfterJobUntil {
                    job_id: JobId::new(JOB_ID).unwrap(),
                    timeout_millis: 30_000
                })
            ),
        ],
    );
    assert_eq!(
        calls
            .runtime
            .iter()
            .filter(|route| **route == "bootstrap")
            .count(),
        1,
    );
    assert_eq!(calls.status_verbose, vec![true]);
    assert_eq!(calls.previews, 0);
    let standard_wait = Some(MutationWaitV1::UntilTerminal {
        timeout_millis: 30_000,
    });
    let expected_mutations = [
        (
            MutationAuthority::Admission,
            "workspace.init",
            "phase5-1",
            standard_wait,
        ),
        (
            MutationAuthority::Admission,
            "session.start",
            "phase5-5",
            standard_wait,
        ),
        (
            MutationAuthority::Admission,
            "session.start_replace",
            "phase5-6",
            standard_wait,
        ),
        (
            MutationAuthority::Admission,
            "session.complete",
            "phase5-9",
            standard_wait,
        ),
        (
            MutationAuthority::Admission,
            "session.skip",
            "phase5-10",
            standard_wait,
        ),
        (
            MutationAuthority::Admission,
            "session.retry",
            "phase5-11",
            standard_wait,
        ),
        (
            MutationAuthority::Admission,
            "session.return",
            "phase5-12",
            standard_wait,
        ),
        (
            MutationAuthority::Admission,
            "session.block",
            "phase5-13",
            standard_wait,
        ),
        (
            MutationAuthority::Admission,
            "session.unblock",
            "phase5-14",
            standard_wait,
        ),
        (
            MutationAuthority::Admission,
            "session.cancel",
            "phase5-15",
            standard_wait,
        ),
        (
            MutationAuthority::Admission,
            "session.reopen",
            "phase5-16",
            standard_wait,
        ),
        (
            MutationAuthority::Admission,
            "session.reset",
            "phase5-17",
            standard_wait,
        ),
        (
            MutationAuthority::ResetAllMaintenance,
            "workspace.reset_all",
            "phase5-18",
            None,
        ),
        (
            MutationAuthority::Admission,
            "item.check",
            "phase5-19",
            standard_wait,
        ),
        (
            MutationAuthority::Admission,
            "item.uncheck",
            "phase5-20",
            standard_wait,
        ),
        (
            MutationAuthority::Admission,
            "item.set",
            "phase5-21",
            standard_wait,
        ),
        (
            MutationAuthority::Admission,
            "item.add",
            "phase5-22",
            standard_wait,
        ),
        (
            MutationAuthority::Admission,
            "item.remove",
            "phase5-23",
            standard_wait,
        ),
        (
            MutationAuthority::Admission,
            "item.attach",
            "phase5-24",
            standard_wait,
        ),
        (
            MutationAuthority::Admission,
            "item.clear",
            "phase5-25",
            standard_wait,
        ),
    ]
    .into_iter()
    .map(|(authority, command, idempotency_key, wait)| MutationCall {
        authority,
        command: command.to_owned(),
        idempotency_key: idempotency_key.to_owned(),
        wait,
    })
    .collect::<Vec<_>>();
    assert_eq!(calls.mutations, expected_mutations);
    assert_eq!(
        calls
            .mutations
            .iter()
            .filter(|call| call.authority == MutationAuthority::ResetAllMaintenance)
            .map(|call| call.command.as_str())
            .collect::<Vec<_>>(),
        vec!["workspace.reset_all"],
    );
    assert!(
        !calls.mutations.iter().any(|call| {
            call.command == "workspace.reset_all" && call.authority == MutationAuthority::Admission
        }),
        "workspace.reset_all must bypass ordinary mutation admission"
    );
}

#[test]
fn dry_run_variants_use_the_readonly_preview_seam_without_mutation() {
    let calls = Arc::new(Mutex::new(Calls::default()));
    let dispatcher = dispatcher(Arc::clone(&calls));
    let cases = vec![
        (
            "session.start",
            json!({
                "selector": selector(),
                "preset": "sw-dev",
                "task_title": "preview start",
                "dry_run": true
            }),
            PreconditionsV1::default(),
        ),
        (
            "session.start_replace",
            json!({
                "selector": selector(),
                "preset": "sw-dev",
                "task_title": "preview replace",
                "dry_run": true
            }),
            session_identity_preconditions(),
        ),
        (
            "session.return",
            json!({
                "selector": selector(),
                "destination_stage_id": "review",
                "reason": "preview return",
                "dry_run": true
            }),
            session_preconditions(),
        ),
        (
            "session.reopen",
            json!({
                "selector": selector(),
                "destination_stage_id": "review",
                "reason": "preview reopen",
                "dry_run": true
            }),
            session_revision_preconditions(),
        ),
        (
            "session.reset",
            json!({"selector": selector(), "dry_run": true}),
            session_identity_preconditions(),
        ),
    ];

    for (index, (command, payload, preconditions)) in cases.into_iter().enumerate() {
        let (request, slice) = request(index as u64 + 70, command, payload, preconditions);
        let response = dispatcher.dispatch(&request, &slice);
        match response {
            ResponseEnvelopeV1::Output(output) => {
                assert_eq!(output.result()["preview"], Value::Bool(true));
                assert!(output.job().is_none());
            }
            ResponseEnvelopeV1::Error(error) => panic!("preview failed: {error:?}"),
        }
    }

    let calls = calls.lock().unwrap();
    assert_eq!(calls.previews, 5);
    assert!(calls.mutations.is_empty());
    assert!(calls.controls.is_empty());
    assert_eq!(
        calls
            .runtime
            .iter()
            .filter(|route| **route == "readonly")
            .count(),
        5,
    );
    assert!(
        calls.runtime.iter().all(|route| *route == "readonly"),
        "a preview must not activate a mutating workspace runtime path"
    );
}
#[test]
fn job_cancel_rejects_nonqueued_preconditions_without_admission() {
    let calls = Arc::new(Mutex::new(Calls::default()));
    let dispatcher = dispatcher(Arc::clone(&calls));
    let (request, slice) = request(
        90,
        "job.cancel",
        json!({"selector": selector(), "job_id": JOB_ID}),
        job_preconditions(JobStateV1::Running),
    );
    let response = dispatcher.dispatch(&request, &slice);
    match response {
        ResponseEnvelopeV1::Error(error) => {
            assert_eq!(error.code().as_str(), "JOB_NOT_CANCELLABLE");
            assert_eq!(error.request_id(), request.request_id());
            assert_eq!(error.command(), request.command());
            assert_eq!(error.exit_code().get(), 1);
        }
        ResponseEnvelopeV1::Output(_) => panic!("running jobs cannot be cancelled"),
    }
    let calls = calls.lock().unwrap();
    assert!(calls.mutations.is_empty());
    assert_eq!(calls.controls, vec![JobStateV1::Running]);
}
#[test]
fn malformed_status_and_next_results_fail_closed_with_correlation() {
    for (sequence, command, payload) in [
        (95, "session.status", json!({"selector": selector()})),
        (96, "session.next", json!({"selector": selector()})),
    ] {
        let calls = Arc::new(Mutex::new(Calls {
            invalid_read_route: Some(if command == "session.status" {
                "status"
            } else {
                "next"
            }),
            ..Calls::default()
        }));
        let dispatcher = dispatcher(Arc::clone(&calls));
        let (request, slice) = request(sequence, command, payload, PreconditionsV1::default());

        match dispatcher.dispatch(&request, &slice) {
            ResponseEnvelopeV1::Error(error) => {
                assert_eq!(error.code().as_str(), "INTERNAL_ERROR");
                assert_eq!(error.request_id(), request.request_id());
                assert_eq!(error.command(), request.command());
            }
            ResponseEnvelopeV1::Output(_) => {
                panic!("invalid {command} result must not be reflected")
            }
        }
        assert!(calls.lock().unwrap().mutations.is_empty());
    }
}
#[test]
fn reset_all_rejects_detach_and_custom_wait_before_maintenance() {
    for (sequence, detach, wait_timeout_ms) in [(100, true, 30_000), (101, false, 1)] {
        let calls = Arc::new(Mutex::new(Calls::default()));
        let dispatcher = dispatcher(Arc::clone(&calls));
        let (request, slice) = request_with_options(
            sequence,
            "workspace.reset_all",
            json!({
                "selector": selector(),
                "confirmed": true,
                "expected_workspace_uuid": WORKSPACE_ID
            }),
            PreconditionsV1::default(),
            detach,
            wait_timeout_ms,
        );

        match dispatcher.dispatch(&request, &slice) {
            ResponseEnvelopeV1::Error(error) => {
                assert_eq!(error.code().as_str(), "REQUEST_INVALID");
                assert_eq!(error.request_id(), request.request_id());
                assert_eq!(error.command(), request.command());
            }
            ResponseEnvelopeV1::Output(_) => panic!("unsupported reset options must fail"),
        }
        assert!(calls.lock().unwrap().mutations.is_empty());
    }
}

#[test]
fn detached_queued_mutation_returns_a_durable_receipt_with_correlation() {
    let calls = Arc::new(Mutex::new(Calls::default()));
    let dispatcher = dispatcher_with_outcome(
        Arc::clone(&calls),
        Ok(MutationDispatchOutcomeV1::Detached { job: queued_job() }),
    );
    let (request, slice) = request_with_options(
        102,
        "session.start",
        json!({"selector": selector(), "preset": "sw-dev", "task_title": "Task"}),
        PreconditionsV1::default(),
        true,
        777,
    );

    match dispatcher.dispatch(&request, &slice) {
        ResponseEnvelopeV1::Output(output) => {
            assert_eq!(output.request_id(), request.request_id());
            assert_eq!(output.command(), request.command());
            assert_eq!(output.job().unwrap().id().as_str(), JOB_ID);
            assert_eq!(output.job().unwrap().state(), JobStateV1::Queued);
            assert_eq!(output.result()["admitted"], Value::Bool(true));
            assert_eq!(output.result()["detached"], Value::Bool(true));
        }
        ResponseEnvelopeV1::Error(error) => panic!("detached admission failed: {error:?}"),
    }
    assert_eq!(
        calls.lock().unwrap().mutations,
        vec![MutationCall {
            authority: MutationAuthority::Admission,
            command: "session.start".to_owned(),
            idempotency_key: "phase5-102".to_owned(),
            wait: Some(MutationWaitV1::Detached),
        }]
    );
}

#[test]
fn synchronous_mutation_honors_custom_wait_and_returns_terminal_output() {
    let calls = Arc::new(Mutex::new(Calls::default()));
    let dispatcher = dispatcher_with_outcome(Arc::clone(&calls), terminal_success());
    let (request, slice) = request_with_options(
        103,
        "session.complete",
        json!({"selector": selector()}),
        session_preconditions(),
        false,
        778,
    );

    match dispatcher.dispatch(&request, &slice) {
        ResponseEnvelopeV1::Output(output) => {
            assert_eq!(output.request_id(), request.request_id());
            assert_eq!(output.command(), request.command());
            assert_eq!(output.job().unwrap().state(), JobStateV1::Succeeded);
            assert_eq!(output.result()["changed"], Value::Bool(true));
        }
        ResponseEnvelopeV1::Error(error) => panic!("synchronous terminal wait failed: {error:?}"),
    }
    assert_eq!(
        calls.lock().unwrap().mutations,
        vec![MutationCall {
            authority: MutationAuthority::Admission,
            command: "session.complete".to_owned(),
            idempotency_key: "phase5-103".to_owned(),
            wait: Some(MutationWaitV1::UntilTerminal {
                timeout_millis: 778,
            }),
        }]
    );
}

#[test]
fn synchronous_timeout_reports_admitted_job_details_with_correlation() {
    let calls = Arc::new(Mutex::new(Calls::default()));
    let dispatcher = dispatcher_with_outcome(
        Arc::clone(&calls),
        Ok(MutationDispatchOutcomeV1::TimedOut { job: queued_job() }),
    );
    let (request, slice) = request_with_options(
        104,
        "session.complete",
        json!({"selector": selector()}),
        session_preconditions(),
        false,
        779,
    );

    match dispatcher.dispatch(&request, &slice) {
        ResponseEnvelopeV1::Error(error) => {
            assert_eq!(error.code().as_str(), "JOB_WAIT_TIMEOUT");
            assert_eq!(error.request_id(), request.request_id());
            assert_eq!(error.command(), request.command());
            assert!(error.retryable());
            assert_eq!(error.exit_code().get(), 4);
            assert_eq!(error.details()["job_id"], Value::String(JOB_ID.to_owned()));
            assert_eq!(error.details()["job_sequence"], Value::from(1));
        }
        ResponseEnvelopeV1::Output(_) => panic!("a synchronous timeout must fail"),
    }
    assert_eq!(
        calls.lock().unwrap().mutations,
        vec![MutationCall {
            authority: MutationAuthority::Admission,
            command: "session.complete".to_owned(),
            idempotency_key: "phase5-104".to_owned(),
            wait: Some(MutationWaitV1::UntilTerminal {
                timeout_millis: 779,
            }),
        }]
    );
}

#[test]
fn terminal_error_preserves_job_details_and_request_correlation() {
    let calls = Arc::new(Mutex::new(Calls::default()));
    let dispatcher = dispatcher_with_outcome(
        Arc::clone(&calls),
        Ok(MutationDispatchOutcomeV1::Terminal {
            job: failed_job(),
            result: DispatcherTerminalResultV1::Error(
                DispatchFailureV1::new(DispatchFailureKindV1::SessionRevisionConflict)
                    .with_details(
                        DispatchErrorDetailsV1::default()
                            .with_expected_revision(Revision::new(1))
                            .with_current_revision(Revision::new(2)),
                    ),
            ),
        }),
    );
    let (request, slice) = request_with_options(
        105,
        "session.complete",
        json!({"selector": selector()}),
        session_preconditions(),
        false,
        780,
    );

    match dispatcher.dispatch(&request, &slice) {
        ResponseEnvelopeV1::Error(error) => {
            assert_eq!(error.code().as_str(), "SESSION_REVISION_CONFLICT");
            assert_eq!(error.request_id(), request.request_id());
            assert_eq!(error.command(), request.command());
            assert!(error.retryable());
            assert_eq!(error.exit_code().get(), 4);
            assert_eq!(error.details()["job_id"], Value::String(JOB_ID.to_owned()));
            assert_eq!(error.details()["job_sequence"], Value::from(1));
            assert_eq!(error.details()["expected_revision"], Value::from(1));
            assert_eq!(error.details()["current_revision"], Value::from(2));
        }
        ResponseEnvelopeV1::Output(_) => panic!("a terminal mutation error must fail"),
    }
    assert_eq!(
        calls.lock().unwrap().mutations,
        vec![MutationCall {
            authority: MutationAuthority::Admission,
            command: "session.complete".to_owned(),
            idempotency_key: "phase5-105".to_owned(),
            wait: Some(MutationWaitV1::UntilTerminal {
                timeout_millis: 780,
            }),
        }]
    );
}

#[test]
fn detached_terminal_replay_preserves_the_immutable_job_and_result() {
    let calls = Arc::new(Mutex::new(Calls::default()));
    let dispatcher = dispatcher_with_outcome(
        Arc::clone(&calls),
        Ok(MutationDispatchOutcomeV1::Terminal {
            job: terminal_job(),
            result: DispatcherTerminalResultV1::Output(DispatcherTerminalOutputV1::new(
                None,
                Map::from_iter([(
                    "replay".to_owned(),
                    Value::String("immutable-terminal-result".to_owned()),
                )]),
                Vec::new(),
            )),
        }),
    );
    let (request, slice) = request_with_options(
        106,
        "session.start",
        json!({"selector": selector(), "preset": "sw-dev", "task_title": "Task"}),
        PreconditionsV1::default(),
        true,
        781,
    );

    let first = match dispatcher.dispatch(&request, &slice) {
        ResponseEnvelopeV1::Output(output) => output,
        ResponseEnvelopeV1::Error(error) => panic!("first detached replay failed: {error:?}"),
    };
    let second = match dispatcher.dispatch(&request, &slice) {
        ResponseEnvelopeV1::Output(output) => output,
        ResponseEnvelopeV1::Error(error) => panic!("second detached replay failed: {error:?}"),
    };
    for output in [&first, &second] {
        assert_eq!(output.request_id(), request.request_id());
        assert_eq!(output.command(), request.command());
        assert_eq!(output.job().unwrap().state(), JobStateV1::Succeeded);
        assert_eq!(output.job().unwrap().id().as_str(), JOB_ID);
        assert_eq!(output.job().unwrap().sequence(), 1);
        assert_eq!(
            output.result()["replay"],
            Value::String("immutable-terminal-result".to_owned())
        );
    }
    assert_eq!(first.job(), second.job());
    assert_eq!(first.result(), second.result());
    assert_eq!(
        calls.lock().unwrap().mutations,
        vec![
            MutationCall {
                authority: MutationAuthority::Admission,
                command: "session.start".to_owned(),
                idempotency_key: "phase5-106".to_owned(),
                wait: Some(MutationWaitV1::Detached),
            },
            MutationCall {
                authority: MutationAuthority::Admission,
                command: "session.start".to_owned(),
                idempotency_key: "phase5-106".to_owned(),
                wait: Some(MutationWaitV1::Detached),
            },
        ]
    );
}

#[test]
fn mutation_wait_and_outcome_mismatches_fail_closed_with_correlation() {
    let cases = vec![
        (
            107,
            true,
            782,
            Ok(MutationDispatchOutcomeV1::TimedOut { job: queued_job() }),
            MutationWaitV1::Detached,
        ),
        (
            108,
            false,
            783,
            Ok(MutationDispatchOutcomeV1::Detached { job: queued_job() }),
            MutationWaitV1::UntilTerminal {
                timeout_millis: 783,
            },
        ),
    ];

    for (sequence, detach, wait_timeout_ms, outcome, expected_wait) in cases {
        let calls = Arc::new(Mutex::new(Calls::default()));
        let dispatcher = dispatcher_with_outcome(Arc::clone(&calls), outcome);
        let (request, slice) = request_with_options(
            sequence,
            "session.start",
            json!({"selector": selector(), "preset": "sw-dev", "task_title": "Task"}),
            PreconditionsV1::default(),
            detach,
            wait_timeout_ms,
        );

        match dispatcher.dispatch(&request, &slice) {
            ResponseEnvelopeV1::Error(error) => {
                assert_eq!(error.code().as_str(), "INTERNAL_ERROR");
                assert_eq!(error.request_id(), request.request_id());
                assert_eq!(error.command(), request.command());
            }
            ResponseEnvelopeV1::Output(_) => {
                panic!("mismatched mutation wait and outcome must fail closed")
            }
        }
        assert_eq!(
            calls.lock().unwrap().mutations,
            vec![MutationCall {
                authority: MutationAuthority::Admission,
                command: "session.start".to_owned(),
                idempotency_key: format!("phase5-{sequence}"),
                wait: Some(expected_wait),
            }]
        );
    }
}
