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

use podway_config::{
    ParsedProcedure, ProcedureDocumentFormat, parse_procedure_document, validate_procedure_v2,
};
use podway_core::{DomainError, JobId, Revision, Sha256Digest, UnixMillis, canonicalize_json_v1};
use podway_daemon::{
    dispatch::{
        CatalogDispatchErrorMapperV1, DevelopmentV2AdmissionProofV1, DispatchFailureKindV1,
        DispatcherWorkspaceOutputV1, MutationAdmissionWorkerV1, RequestDispatcherV1Adapter,
        WorkspaceRuntimeV1,
    },
    production::{
        NativeProductionClockV1, ProductionControlServiceV1, ProductionMutationWorkerV1,
        ProductionPreviewServiceV1, ProductionReadServiceV1, ProductionWorkspaceRuntimeV1,
        ProductionWorkspaceV1, compose_dispatcher_v1, compose_dispatcher_with_worker_v1,
    },
    runtime_workspace::WorkspaceRuntimeObservationV1,
    server::{DaemonRequestV1, RequestDispatcherV1},
};
use podway_git::{GitResolverContractV1, NativeGitResolverV1};
use podway_protocol::{
    ClientInfoV1, CommandNameV1, IdempotencyKeyV1, JobStateV1, NextResultV1, OperationV1,
    OutputEnvelopeV1, PreconditionsV1, RequestEnvelopeInputV1, RequestEnvelopeV1, RequestIdV1,
    RequestOptionsV1, ResponseEnvelopeV1, ResponseEnvelopeV2, Rfc3339MillisV1, SliceRequestV1,
    StageStatusResultV1, StatusItemValueV1, StatusResultV1, WorkspaceContextV1,
    WorktreeSelectorWireV1,
};
use podway_service::ServiceRuntimePathsV1;
use podway_store::{
    AdmissionSessionIdentityV1, AdmitOutcomeV1, AdmitRequestV1, CommandV1,
    IdempotencyKeyV1 as StoreIdempotencyKeyV1, JobListQueryV1, PersistedResponseContextV1,
    RevisionAttemptItemPreconditionsV1, SqliteStoreOptionsV1, SqliteStoreV1, StoreContractV1,
    StoreGraphStateContractV2, StoreReadContractV1, TerminalResultV1, WorkerIdV1,
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

    fn development_v2_admission(
        &self,
        _selector: &WorktreeSelectorWireV1,
    ) -> Option<DevelopmentV2AdmissionProofV1> {
        Some(DevelopmentV2AdmissionProofV1::granted_for_runtime())
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

fn development_v2_dispatcher(
    manager: Arc<podway_daemon::runtime_workspace::WorkspaceRuntimeManagerV1>,
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
        "session.start" | "session.start_replace"
            if payload.get("dry_run").and_then(Value::as_bool) == Some(true) =>
        {
            OperationV1::Query
        }
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
fn v2run001_production_custom_start_dry_run_live_and_source_independent_replay() {
    let fixture = git_worktrees();
    make_runtime_private(fixture.main());
    let runtime_manager = Arc::new(manager(fixture.temporary_path()));
    let composition = compose_dispatcher_with_worker_v1(
        Arc::clone(&runtime_manager),
        WorkerIdV1::new("v2run001-production-start").unwrap(),
    );
    let workspace_selector = selector(fixture.main());
    let initialize = request(
        10_001,
        "workspace.init",
        &workspace_selector,
        json!({"selector": serde_json::to_value(&workspace_selector).unwrap()}),
        RequestOptionsV1::new(false, 5_000).unwrap(),
        "v2run001-initialize",
        PreconditionsV1::default(),
    );
    dispatch_command(composition.dispatcher(), &initialize, "workspace.init");

    let source = include_bytes!("../../../tests/fixtures/v2/procedures/equivalent-procedure.yaml");
    fs::write(fixture.main().join("workflow.yaml"), source).unwrap();
    let ParsedProcedure::V2(parsed) =
        parse_procedure_document(source, ProcedureDocumentFormat::Yaml).unwrap()
    else {
        panic!("the V2RUN-001 production fixture must be Procedure v2")
    };
    let digest = validate_procedure_v2(parsed).unwrap().digest().clone();
    let start_payload = json!({
        "selector": serde_json::to_value(&workspace_selector).unwrap(),
        "procedure": "workflow.yaml",
        "expected_procedure_digest": digest,
        "task_title": "Production Procedure v2 start"
    });
    let runtime = runtime_manager
        .resolve_existing(git_selector(fixture.main()), None, observation())
        .unwrap();
    let context = runtime.context_snapshot();
    let identity = context.binding().identity();
    let direct_store = SqliteStoreV1::open(
        context.database_path(),
        context.workspace_root(),
        identity.clone(),
        context.store_options().clone(),
        UnixMillis::new(1),
    )
    .unwrap();
    let baseline_sequence = direct_store
        .read_workspace_view(identity)
        .unwrap()
        .latest_workspace_sequence();
    let baseline_jobs = direct_store
        .list_jobs(identity, JobListQueryV1::new(100).unwrap())
        .unwrap()
        .len();
    let baseline_registry = runtime_manager
        .registry()
        .lookup(identity.workspace_uuid())
        .unwrap();
    let readonly_before = runtime_manager
        .resolve_existing_readonly(git_selector(fixture.main()), None)
        .unwrap();
    assert!(baseline_registry.is_some());
    assert!(readonly_before.active_scheduler().is_some());

    fs::write(
        fixture.main().join("legacy.yaml"),
        include_bytes!("../../../assets/presets/sw-dev.yaml"),
    )
    .unwrap();
    let legacy = request(
        10_018,
        "session.start",
        &workspace_selector,
        json!({
            "selector": serde_json::to_value(&workspace_selector).unwrap(),
            "procedure": "legacy.yaml",
            "task_title": "Retained v1 fallback",
            "dry_run": true,
        }),
        RequestOptionsV1::new(false, 5_000).unwrap(),
        "unused-v1-dry-run-key",
        PreconditionsV1::default(),
    );
    let legacy_daemon = DaemonRequestV1::from_envelope(&legacy.0).unwrap();
    assert!(
        composition
            .worker()
            .dispatch_development_v2(
                DevelopmentV2AdmissionProofV1::granted_for_runtime(),
                &legacy.0,
                &legacy_daemon,
            )
            .unwrap()
            .is_none(),
        "retained v1 starts must fall through the development v2 probe"
    );
    assert_eq!(
        direct_store
            .list_jobs(identity, JobListQueryV1::new(100).unwrap())
            .unwrap()
            .len(),
        baseline_jobs
    );

    let mut dry_run_payload = start_payload.clone();
    dry_run_payload["dry_run"] = Value::Bool(true);
    let dry_run = request(
        10_002,
        "session.start",
        &workspace_selector,
        dry_run_payload,
        RequestOptionsV1::new(false, 5_000).unwrap(),
        "v2run001-dry-run",
        PreconditionsV1::default(),
    );
    let dry_run_daemon = DaemonRequestV1::from_envelope(&dry_run.0).unwrap();
    let dry_run_response = composition
        .worker()
        .dispatch_development_v2(
            DevelopmentV2AdmissionProofV1::granted_for_runtime(),
            &dry_run.0,
            &dry_run_daemon,
        )
        .unwrap()
        .expect("a confirmed custom Procedure v2 dry-run must be handled");
    let ResponseEnvelopeV2::OutputV2(dry_run_output) = dry_run_response else {
        panic!("Procedure v2 dry-run must return podway.output/v2")
    };
    assert_eq!(dry_run_output.result()["dry_run"], true);
    assert!(dry_run_output.job().is_none());
    assert!(dry_run_output.result().get("admission").is_none());
    for forbidden in ["session_id", "revision", "entry_graph_node_id"] {
        assert!(
            dry_run_output.result().get(forbidden).is_none(),
            "dry-run must omit {forbidden}"
        );
    }

    assert!(
        direct_store
            .read_graph_session_v2(identity)
            .unwrap()
            .is_none(),
        "dry-run must not create graph state"
    );
    assert_eq!(
        direct_store
            .read_workspace_view(identity)
            .unwrap()
            .latest_workspace_sequence(),
        baseline_sequence
    );
    assert_eq!(
        direct_store
            .list_jobs(identity, JobListQueryV1::new(100).unwrap())
            .unwrap()
            .len(),
        baseline_jobs
    );
    assert_eq!(
        runtime_manager
            .registry()
            .lookup(identity.workspace_uuid())
            .unwrap(),
        baseline_registry
    );
    assert!(
        runtime_manager
            .resolve_existing_readonly(git_selector(fixture.main()), None)
            .unwrap()
            .active_scheduler()
            .is_some()
    );

    for (request_number, key, expected, payload) in [
        (
            10_010,
            "v2run001-missing-digest",
            DispatchFailureKindV1::DigestConfirmationRequired,
            {
                let mut payload = start_payload.clone();
                payload
                    .as_object_mut()
                    .unwrap()
                    .remove("expected_procedure_digest");
                payload
            },
        ),
        (
            10_011,
            "v2run001-wrong-digest",
            DispatchFailureKindV1::ProcedureDigestMismatch,
            {
                let mut payload = start_payload.clone();
                payload["expected_procedure_digest"] = json!(format!("sha256:{}", "b".repeat(64)));
                payload
            },
        ),
    ] {
        let rejected = request(
            request_number,
            "session.start",
            &workspace_selector,
            payload,
            RequestOptionsV1::new(false, 5_000).unwrap(),
            key,
            PreconditionsV1::default(),
        );
        let rejected_daemon = DaemonRequestV1::from_envelope(&rejected.0).unwrap();
        let failure = composition
            .worker()
            .dispatch_development_v2(
                DevelopmentV2AdmissionProofV1::granted_for_runtime(),
                &rejected.0,
                &rejected_daemon,
            )
            .unwrap_err();
        assert_eq!(failure.kind(), expected);
        assert!(
            direct_store
                .read_graph_session_v2(identity)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            direct_store
                .list_jobs(identity, JobListQueryV1::new(100).unwrap())
                .unwrap()
                .len(),
            baseline_jobs
        );
    }

    let live = request(
        10_003,
        "session.start",
        &workspace_selector,
        start_payload.clone(),
        RequestOptionsV1::new(false, 5_000).unwrap(),
        "v2run001-live",
        PreconditionsV1::default(),
    );
    let live_daemon = DaemonRequestV1::from_envelope(&live.0).unwrap();
    let live_response = composition
        .worker()
        .dispatch_development_v2(
            DevelopmentV2AdmissionProofV1::granted_for_runtime(),
            &live.0,
            &live_daemon,
        )
        .unwrap()
        .expect("a confirmed custom Procedure v2 start must be handled");
    let ResponseEnvelopeV2::OutputV2(live_output) = &live_response else {
        panic!("Procedure v2 live start must return podway.output/v2")
    };
    assert_eq!(live_output.request_id(), live.0.request_id());
    assert_eq!(live_output.command(), live.0.command());
    assert_eq!(live_output.result()["dry_run"], false);
    assert_eq!(live_output.result()["admission"]["admitted"], true);
    assert!(live_output.job().is_some());
    assert_eq!(live_output.result()["revision"], 1);

    let session_id = live_output.result()["session_id"].as_str().unwrap();
    let session_id_value = podway_core::SessionId::new(session_id).unwrap();
    let graph_before_reads = direct_store
        .read_graph_session_v2(identity)
        .unwrap()
        .unwrap();
    let attempt_id = graph_before_reads
        .trace()
        .active_attempt()
        .unwrap()
        .attempt_id()
        .as_str()
        .to_owned();
    let sequence_before_reads = direct_store
        .read_workspace_view(identity)
        .unwrap()
        .latest_workspace_sequence();
    let jobs_before_reads = direct_store
        .list_jobs(identity, JobListQueryV1::new(100).unwrap())
        .unwrap()
        .len();
    let read_preconditions =
        PreconditionsV1::new(Some(session_id_value.clone()), None, None, None, None, None).unwrap();
    let routed_reads =
        development_v2_dispatcher(Arc::clone(&runtime_manager), "v2run002-version-routing");

    for (request_number, payload, expected_schema, expected_tier) in [
        (
            10_020,
            json!({"selector": serde_json::to_value(&workspace_selector).unwrap()}),
            "podway.status-result/v2",
            Some("standard"),
        ),
        (
            10_021,
            json!({
                "selector": serde_json::to_value(&workspace_selector).unwrap(),
                "wait_for_idle": true,
                "compact": true,
            }),
            "podway.compact-status-result/v2",
            None,
        ),
        (
            10_022,
            json!({
                "selector": serde_json::to_value(&workspace_selector).unwrap(),
                "verbose": true,
                "history_before": 1,
            }),
            "podway.status-result/v2",
            Some("verbose"),
        ),
    ] {
        let status = request(
            request_number,
            "session.status",
            &workspace_selector,
            payload,
            RequestOptionsV1::new(false, 5_000).unwrap(),
            "unused-v2-status-key",
            read_preconditions.clone(),
        );
        let daemon_request = DaemonRequestV1::from_envelope(&status.0).unwrap();
        let response = routed_reads.dispatch_daemon(&status.0, &daemon_request);
        let ResponseEnvelopeV2::OutputV2(output) = response else {
            panic!("the gated dispatcher must select podway.output/v2 for Procedure v2 status")
        };
        assert_eq!(output.command().as_str(), "session.status");
        assert_eq!(output.result()["schema"], expected_schema);
        assert_eq!(output.result()["session"]["id"], session_id);
        assert_eq!(output.result()["session"]["revision"], 1);
        assert_eq!(
            output.result().get("tier").and_then(Value::as_str),
            expected_tier
        );
        assert_eq!(
            output.result()["queue"]["latest_workspace_sequence"],
            sequence_before_reads
        );
        if expected_tier == Some("verbose") {
            assert!(
                output.result()["current_trace_history"]["entries"]
                    .as_array()
                    .unwrap()
                    .is_empty()
            );
            assert_eq!(
                output.result()["current_trace_history"]["trace_truncated"],
                false
            );
        }
    }

    let next = request(
        10_023,
        "session.next",
        &workspace_selector,
        json!({"selector": serde_json::to_value(&workspace_selector).unwrap()}),
        RequestOptionsV1::new(false, 5_000).unwrap(),
        "unused-v2-next-key",
        read_preconditions.clone(),
    );
    let next_daemon = DaemonRequestV1::from_envelope(&next.0).unwrap();
    let next_response = routed_reads.dispatch_daemon(&next.0, &next_daemon);
    let ResponseEnvelopeV2::OutputV2(next_output) = next_response else {
        panic!("the gated dispatcher must select podway.output/v2 for Procedure v2 next")
    };
    assert_eq!(next_output.result()["schema"], "podway.next-result/v2");
    assert_eq!(next_output.result()["node"]["graph_node_id"], "perform");
    assert_eq!(next_output.result()["attempt"]["attempt_id"], attempt_id);
    assert_eq!(next_output.result()["revision"], 1);
    assert_eq!(
        next_output.result()["queue"]["latest_workspace_sequence"],
        sequence_before_reads
    );

    let wrong_session_status = request(
        10_024,
        "session.status",
        &workspace_selector,
        json!({"selector": serde_json::to_value(&workspace_selector).unwrap()}),
        RequestOptionsV1::new(false, 5_000).unwrap(),
        "unused-v2-wrong-session-key",
        PreconditionsV1::new(
            Some(podway_core::SessionId::new("00000000-0000-4000-8000-000000010024").unwrap()),
            None,
            None,
            None,
            None,
            None,
        )
        .unwrap(),
    );
    let wrong_session_daemon = DaemonRequestV1::from_envelope(&wrong_session_status.0).unwrap();
    assert_eq!(
        composition
            .worker()
            .dispatch_development_v2(
                DevelopmentV2AdmissionProofV1::granted_for_runtime(),
                &wrong_session_status.0,
                &wrong_session_daemon,
            )
            .unwrap_err()
            .kind(),
        DispatchFailureKindV1::SessionIdMismatch
    );
    assert_eq!(
        direct_store.read_graph_session_v2(identity).unwrap(),
        Some(graph_before_reads),
        "Procedure v2 reads must not mutate the graph"
    );
    assert_eq!(
        direct_store
            .read_workspace_view(identity)
            .unwrap()
            .latest_workspace_sequence(),
        sequence_before_reads,
        "Procedure v2 reads must not advance workspace sequence"
    );
    assert_eq!(
        direct_store
            .list_jobs(identity, JobListQueryV1::new(100).unwrap())
            .unwrap()
            .len(),
        jobs_before_reads,
        "Procedure v2 reads must not admit jobs"
    );

    let jobs_before_replace_dry_run = direct_store
        .list_jobs(identity, JobListQueryV1::new(100).unwrap())
        .unwrap()
        .len();
    let mut replace_dry_run_payload = start_payload.clone();
    replace_dry_run_payload["dry_run"] = Value::Bool(true);
    let replace_dry_run = request(
        10_014,
        "session.start_replace",
        &workspace_selector,
        replace_dry_run_payload.clone(),
        RequestOptionsV1::new(false, 5_000).unwrap(),
        "unused-dry-run-key",
        PreconditionsV1::new(
            Some(podway_core::SessionId::new(session_id).unwrap()),
            Some(Revision::new(1)),
            None,
            None,
            None,
            None,
        )
        .unwrap(),
    );
    let replace_dry_run_daemon = DaemonRequestV1::from_envelope(&replace_dry_run.0).unwrap();
    let replace_dry_run_response = composition
        .worker()
        .dispatch_development_v2(
            DevelopmentV2AdmissionProofV1::granted_for_runtime(),
            &replace_dry_run.0,
            &replace_dry_run_daemon,
        )
        .unwrap()
        .expect("an exactly fenced replacement dry-run must be handled");
    let ResponseEnvelopeV2::OutputV2(replace_dry_run_output) = replace_dry_run_response else {
        panic!("Procedure v2 replacement dry-run must return podway.output/v2")
    };
    assert_eq!(replace_dry_run_output.result()["dry_run"], true);

    let stale_replace_dry_run = request(
        10_015,
        "session.start_replace",
        &workspace_selector,
        replace_dry_run_payload,
        RequestOptionsV1::new(false, 5_000).unwrap(),
        "unused-stale-dry-run-key",
        PreconditionsV1::new(
            Some(podway_core::SessionId::new(session_id).unwrap()),
            Some(Revision::new(2)),
            None,
            None,
            None,
            None,
        )
        .unwrap(),
    );
    let stale_replace_dry_run_daemon =
        DaemonRequestV1::from_envelope(&stale_replace_dry_run.0).unwrap();
    let stale_dry_run_failure = composition
        .worker()
        .dispatch_development_v2(
            DevelopmentV2AdmissionProofV1::granted_for_runtime(),
            &stale_replace_dry_run.0,
            &stale_replace_dry_run_daemon,
        )
        .unwrap_err();
    assert_eq!(
        stale_dry_run_failure.kind(),
        DispatchFailureKindV1::SessionRevisionConflict
    );
    assert_eq!(
        direct_store
            .list_jobs(identity, JobListQueryV1::new(100).unwrap())
            .unwrap()
            .len(),
        jobs_before_replace_dry_run,
        "replacement dry-runs must not create durable jobs"
    );
    assert_eq!(
        direct_store
            .read_graph_session_v2(identity)
            .unwrap()
            .unwrap()
            .trace()
            .session_id()
            .as_str(),
        session_id
    );

    let mut replace_payload = start_payload.clone();
    replace_payload["task_title"] = json!("Replacement Procedure v2 start");
    replace_payload["confirmed"] = Value::Bool(true);
    let jobs_before_stale_replace = direct_store
        .list_jobs(identity, JobListQueryV1::new(100).unwrap())
        .unwrap()
        .len();
    let wrong_identity_replace = request(
        10_016,
        "session.start_replace",
        &workspace_selector,
        replace_payload.clone(),
        RequestOptionsV1::new(false, 5_000).unwrap(),
        "v2run001-wrong-identity-replace",
        PreconditionsV1::new(
            Some(podway_core::SessionId::new("00000000-0000-4000-8000-000000010016").unwrap()),
            Some(Revision::new(2)),
            None,
            None,
            None,
            None,
        )
        .unwrap(),
    );
    let wrong_identity_replace_daemon =
        DaemonRequestV1::from_envelope(&wrong_identity_replace.0).unwrap();
    let wrong_identity_failure = composition
        .worker()
        .dispatch_development_v2(
            DevelopmentV2AdmissionProofV1::granted_for_runtime(),
            &wrong_identity_replace.0,
            &wrong_identity_replace_daemon,
        )
        .unwrap_err();
    assert_eq!(
        wrong_identity_failure.kind(),
        DispatchFailureKindV1::SessionIdMismatch
    );
    let stale_replace = request(
        10_013,
        "session.start_replace",
        &workspace_selector,
        replace_payload.clone(),
        RequestOptionsV1::new(false, 5_000).unwrap(),
        "v2run001-stale-replace",
        PreconditionsV1::new(
            Some(podway_core::SessionId::new(session_id).unwrap()),
            Some(Revision::new(2)),
            None,
            None,
            None,
            None,
        )
        .unwrap(),
    );
    let stale_replace_daemon = DaemonRequestV1::from_envelope(&stale_replace.0).unwrap();
    let stale_failure = composition
        .worker()
        .dispatch_development_v2(
            DevelopmentV2AdmissionProofV1::granted_for_runtime(),
            &stale_replace.0,
            &stale_replace_daemon,
        )
        .unwrap_err();
    assert_eq!(
        stale_failure.kind(),
        DispatchFailureKindV1::SessionRevisionConflict
    );
    assert_eq!(
        direct_store
            .list_jobs(identity, JobListQueryV1::new(100).unwrap())
            .unwrap()
            .len(),
        jobs_before_stale_replace,
        "a stale replacement must fail before durable admission"
    );
    assert_eq!(
        direct_store
            .read_graph_session_v2(identity)
            .unwrap()
            .unwrap()
            .trace()
            .session_id()
            .as_str(),
        session_id
    );
    let replace = request(
        10_004,
        "session.start_replace",
        &workspace_selector,
        replace_payload,
        RequestOptionsV1::new(false, 5_000).unwrap(),
        "v2run001-replace",
        PreconditionsV1::new(
            Some(podway_core::SessionId::new(session_id).unwrap()),
            Some(Revision::new(1)),
            None,
            None,
            None,
            None,
        )
        .unwrap(),
    );
    let replace_daemon = DaemonRequestV1::from_envelope(&replace.0).unwrap();
    let replace_response = composition
        .worker()
        .dispatch_development_v2(
            DevelopmentV2AdmissionProofV1::granted_for_runtime(),
            &replace.0,
            &replace_daemon,
        )
        .unwrap()
        .expect("confirmed v2 replacement must be handled");
    let ResponseEnvelopeV2::OutputV2(replace_output) = replace_response else {
        panic!("Procedure v2 replacement must return podway.output/v2")
    };
    assert_eq!(replace_output.result()["revision"], 1);
    assert_ne!(replace_output.result()["session_id"], session_id);

    fs::remove_file(fixture.main().join("workflow.yaml")).unwrap();
    let replay = composition
        .worker()
        .dispatch_development_v2(
            DevelopmentV2AdmissionProofV1::granted_for_runtime(),
            &live.0,
            &live_daemon,
        )
        .unwrap()
        .expect("an exact replay must not reread a deleted source");
    assert_eq!(
        serde_json::to_value(replay).unwrap(),
        serde_json::to_value(live_response).unwrap(),
        "exact replay must preserve the frozen v2 response"
    );
}

#[test]
fn inactive_v2_after_job_reads_terminal_state_without_activating_scheduler() {
    let fixture = git_worktrees();
    make_runtime_private(fixture.main());
    let workspace_selector = selector(fixture.main());
    let (terminal_job_id, session_id) = {
        let runtime_manager = Arc::new(manager(fixture.temporary_path()));
        let composition = compose_dispatcher_with_worker_v1(
            Arc::clone(&runtime_manager),
            WorkerIdV1::new("v2run002-inactive-initializer").unwrap(),
        );
        let initialize = request(
            10_030,
            "workspace.init",
            &workspace_selector,
            json!({"selector": serde_json::to_value(&workspace_selector).unwrap()}),
            RequestOptionsV1::new(false, 5_000).unwrap(),
            "v2run002-inactive-initialize",
            PreconditionsV1::default(),
        );
        dispatch_command(composition.dispatcher(), &initialize, "workspace.init");

        let source =
            include_bytes!("../../../tests/fixtures/v2/procedures/equivalent-procedure.yaml");
        fs::write(fixture.main().join("inactive-workflow.yaml"), source).unwrap();
        let ParsedProcedure::V2(parsed) =
            parse_procedure_document(source, ProcedureDocumentFormat::Yaml).unwrap()
        else {
            panic!("the inactive read fixture must be Procedure v2")
        };
        let procedure_digest = validate_procedure_v2(parsed).unwrap().digest().clone();
        let start = request(
            10_031,
            "session.start",
            &workspace_selector,
            json!({
                "selector": serde_json::to_value(&workspace_selector).unwrap(),
                "procedure": "inactive-workflow.yaml",
                "expected_procedure_digest": procedure_digest,
                "task_title": "Inactive Procedure v2 read"
            }),
            RequestOptionsV1::new(false, 5_000).unwrap(),
            "v2run002-inactive-start",
            PreconditionsV1::default(),
        );
        let daemon_request = DaemonRequestV1::from_envelope(&start.0).unwrap();
        let response = composition
            .worker()
            .dispatch_development_v2(
                DevelopmentV2AdmissionProofV1::granted_for_runtime(),
                &start.0,
                &daemon_request,
            )
            .unwrap()
            .expect("the Procedure v2 start must be handled");
        let ResponseEnvelopeV2::OutputV2(output) = response else {
            panic!("Procedure v2 start must return podway.output/v2")
        };
        assert_eq!(output.job().unwrap().state(), JobStateV1::Succeeded);
        fs::remove_file(fixture.main().join("inactive-workflow.yaml")).unwrap();
        (
            output.job().unwrap().id().clone(),
            podway_core::SessionId::new(output.result()["session_id"].as_str().unwrap()).unwrap(),
        )
    };

    let runtime_manager = Arc::new(manager(fixture.temporary_path()));
    assert!(
        runtime_manager
            .resolve_existing_readonly(git_selector(fixture.main()), None)
            .unwrap()
            .active_scheduler()
            .is_none()
    );
    let composition = compose_dispatcher_with_worker_v1(
        Arc::clone(&runtime_manager),
        WorkerIdV1::new("v2run002-inactive-reader").unwrap(),
    );
    let routed_reads = development_v2_dispatcher(
        Arc::clone(&runtime_manager),
        "v2run002-inactive-version-routing",
    );
    let preconditions =
        PreconditionsV1::new(Some(session_id), None, None, None, None, None).unwrap();
    let status = request(
        10_032,
        "session.status",
        &workspace_selector,
        json!({
            "selector": serde_json::to_value(&workspace_selector).unwrap(),
            "after_job_id": terminal_job_id,
        }),
        RequestOptionsV1::new(false, 100).unwrap(),
        "unused-inactive-status",
        preconditions.clone(),
    );
    let daemon_request = DaemonRequestV1::from_envelope(&status.0).unwrap();
    let response = routed_reads.dispatch_daemon(&status.0, &daemon_request);
    let ResponseEnvelopeV2::OutputV2(output) = response else {
        panic!("the restarted gated dispatcher must return podway.output/v2")
    };
    assert_eq!(output.result()["schema"], "podway.status-result/v2");
    assert_eq!(output.result()["procedure"]["id"], "fixture-equivalence");

    let unknown = request(
        10_033,
        "session.status",
        &workspace_selector,
        json!({
            "selector": serde_json::to_value(&workspace_selector).unwrap(),
            "after_job_id": "00000000-0000-4000-8000-000000009999",
        }),
        RequestOptionsV1::new(false, 100).unwrap(),
        "unused-inactive-status",
        preconditions,
    );
    let daemon_request = DaemonRequestV1::from_envelope(&unknown.0).unwrap();
    let failure = composition
        .worker()
        .dispatch_development_v2(
            DevelopmentV2AdmissionProofV1::granted_for_runtime(),
            &unknown.0,
            &daemon_request,
        )
        .unwrap_err();
    assert_eq!(failure.kind(), DispatchFailureKindV1::JobNotFound);
    assert!(
        runtime_manager
            .resolve_existing_readonly(git_selector(fixture.main()), None)
            .unwrap()
            .active_scheduler()
            .is_none(),
        "inactive reads must not create a scheduler generation"
    );
}

#[test]
fn production_composition_bootstraps_replays_and_covers_active_attempt_retry_return() {
    let fixture = git_worktrees();
    make_runtime_private(fixture.main());
    let manager = Arc::new(manager(fixture.temporary_path()));
    let dispatcher = development_v2_dispatcher(Arc::clone(&manager), "production-composition-test");
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
    assert!(
        initialized
            .workspace()
            .expect("workspace.init must return workspace metadata")
            .latest_workspace_sequence()
            >= initialized.job().unwrap().sequence(),
        "terminal workspace metadata must include its own admission sequence"
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
    assert!(
        started
            .workspace()
            .expect("session.start must return workspace metadata")
            .latest_workspace_sequence()
            >= started.job().unwrap().sequence(),
        "terminal workspace metadata must include its own admission sequence"
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
    assert_eq!(
        missing_lookup.result()["schema"],
        "podway.job-lookup-result/v1"
    );
    assert_eq!(missing_lookup.result().len(), 2);
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
    let status_daemon = DaemonRequestV1::from_envelope(&status.0).unwrap();
    let ResponseEnvelopeV2::OutputV1(initial_status_output) =
        dispatcher.dispatch_daemon(&status.0, &status_daemon)
    else {
        panic!("a retained v1 session must keep podway.output/v1 status")
    };
    let initial_status = status_result(&initial_status_output);
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
    let next_daemon = DaemonRequestV1::from_envelope(&next.0).unwrap();
    let ResponseEnvelopeV2::OutputV1(initial_next_output) =
        dispatcher.dispatch_daemon(&next.0, &next_daemon)
    else {
        panic!("a retained v1 session must keep podway.output/v1 next")
    };
    let initial_next = next_result(&initial_next_output);
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
        Some(StatusItemValueV1::Text(
            "discard this value on clean retry".to_owned()
        ))
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
        None,
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
        None,
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
fn aut_t_recon_missing_lookup_before_workspace_init_is_read_only_and_returns_not_found() {
    let fixture = git_worktrees();
    make_runtime_private(fixture.main());
    let runtime_manager = Arc::new(manager(fixture.temporary_path()));
    let dispatcher = compose_dispatcher_v1(
        Arc::clone(&runtime_manager),
        WorkerIdV1::new("recon-uninitialized-lookup").unwrap(),
    );
    let workspace_selector = selector(fixture.main());
    let runtime_directory = fixture.main().join(".podway/runtime");
    let entries_before = fs::read_dir(&runtime_directory)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<Vec<_>>();

    let lookup = request(
        918,
        "job.lookup",
        &workspace_selector,
        json!({
            "selector": serde_json::to_value(&workspace_selector).unwrap(),
            "idempotency_key": "workspace-init-response-never-admitted",
        }),
        RequestOptionsV1::new(false, 0).unwrap(),
        "unused-query-key",
        PreconditionsV1::default(),
    );
    let lookup = dispatch_command(&dispatcher, &lookup, "job.lookup");

    assert_eq!(lookup.result()["found"], false);
    assert!(lookup.workspace().is_none());
    assert!(!runtime_directory.join("state.sqlite3").exists());
    let entries_after = fs::read_dir(&runtime_directory)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect::<Vec<_>>();
    assert_eq!(entries_after, entries_before);
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
        &json!({
            "schema": "podway.job-lookup-result/v1",
            "found": false
        })
        .as_object()
        .unwrap()
        .clone()
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

#[test]
fn job_lookup_on_inactive_workspace_leaves_store_registry_and_scheduler_unchanged() {
    let fixture = git_worktrees();
    make_runtime_private(fixture.main());
    let first_manager = Arc::new(manager(fixture.temporary_path()));
    let first_dispatcher = compose_dispatcher_v1(
        Arc::clone(&first_manager),
        WorkerIdV1::new("inactive-recon-initializer").unwrap(),
    );
    let workspace_selector = selector(fixture.main());
    let initialize = request(
        941,
        "workspace.init",
        &workspace_selector,
        json!({"selector": serde_json::to_value(&workspace_selector).unwrap()}),
        RequestOptionsV1::new(false, 5_000).unwrap(),
        "inactive-recon-key",
        PreconditionsV1::default(),
    );
    let initialized = dispatch_command(&first_dispatcher, &initialize, "workspace.init");
    assert_eq!(initialized.job().unwrap().state(), JobStateV1::Succeeded);
    drop(first_dispatcher);
    drop(first_manager);

    let runtime_manager = Arc::new(manager(fixture.temporary_path()));
    let before_resolution = runtime_manager
        .resolve_existing_readonly(git_selector(fixture.main()), None)
        .expect("inactive workspace must resolve without activation");
    assert!(before_resolution.active_scheduler().is_none());
    let database_path = before_resolution.database_path().to_path_buf();
    let registry_path = runtime_manager.registry().registry_path().to_path_buf();
    let idempotency_key = StoreIdempotencyKeyV1::new("inactive-recon-key").unwrap();
    let store_before = SqliteStoreV1::inspect_reconciliation_snapshot(
        &database_path,
        before_resolution.binding().identity(),
        before_resolution.store_options(),
        &idempotency_key,
        UnixMillis::UNIX_EPOCH,
    )
    .expect("inactive store must be inspectable");
    let registry_before = fs::read(&registry_path).expect("registry must be readable");
    let dispatcher = compose_dispatcher_v1(
        Arc::clone(&runtime_manager),
        WorkerIdV1::new("inactive-recon-reader").unwrap(),
    );
    let lookup = request(
        942,
        "job.lookup",
        &workspace_selector,
        json!({
            "selector": serde_json::to_value(&workspace_selector).unwrap(),
            "idempotency_key": "inactive-recon-key",
        }),
        RequestOptionsV1::new(false, 0).unwrap(),
        "unused-query-key",
        PreconditionsV1::default(),
    );
    let lookup = dispatch_command(&dispatcher, &lookup, "job.lookup");
    assert_eq!(lookup.result()["found"], true);
    assert_eq!(lookup.result()["job"]["state"], "succeeded");
    let after_resolution = runtime_manager
        .resolve_existing_readonly(git_selector(fixture.main()), None)
        .expect("lookup must leave the workspace resolvable");
    assert!(
        after_resolution.active_scheduler().is_none(),
        "read-only reconciliation must not create a scheduler"
    );
    let store_after = SqliteStoreV1::inspect_reconciliation_snapshot(
        &database_path,
        after_resolution.binding().identity(),
        after_resolution.store_options(),
        &idempotency_key,
        UnixMillis::UNIX_EPOCH,
    )
    .expect("inactive store must remain inspectable");
    assert_eq!(store_after, store_before);
    assert_eq!(
        fs::read(&registry_path).expect("registry must remain readable"),
        registry_before
    );
}
