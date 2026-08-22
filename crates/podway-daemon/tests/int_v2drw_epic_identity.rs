//! Real-SQLite regressions for post-admission Procedure v2 session identity drift.

use super::{int_v2run003_runtime as runtime, support_phase4_workspace};

use std::{path::PathBuf, sync::Arc};

use podway_config::{
    ParsedProcedure, ProcedureDocumentFormat, parse_procedure_document, validate_procedure_v2,
};
use podway_core::{AttemptId, DomainError, Revision, SessionId, TerminalDispositionV2, UnixMillis};
use podway_daemon::{
    execution::{
        DaemonExecutionEngineV1, ExecutionClockV1,
        graph_prepared_session_state_from_procedure_v2_snapshot,
        graph_session_state_from_procedure_v2_snapshot, workspace_procedure_snapshot_from_bytes_v2,
    },
    native_execution::{
        NativeArtifactVerifierV1, NativeExecutionIdSourceV1, NativeProcedureProviderV1,
        NativeWorkspaceRevalidatorV1, WallUtcExecutionClockV1,
    },
    server::DaemonRequestV1,
    workspace::{SqliteWorkspaceBindingInspectorV1, WorkspaceResolverV1},
};
use podway_git::NativeGitResolverV1;
use podway_protocol::{
    ClientInfoV1, CommandNameV1, IdempotencyKeyV1 as ProtocolIdempotencyKeyV1, OperationV1,
    PreconditionsV1, ProcedureV2MutationRequestV1, ProcedureV2StartRequestV1,
    RequestEnvelopeInputV1, RequestEnvelopeV1, RequestIdV1, RequestOptionsV1, SliceRequestV1,
    WorkspaceContextV1, WorktreeSelectorWireV1,
};
use podway_store::{
    AdmissionSessionIdentityV1, AdmitOutcomeV1, AdmitRequestV1, CanonicalExecutionJsonV1,
    CommandV1, IdempotencyKeyV1, JobIdV1, JobReceiptOrTerminalV1, JobStateV1,
    PersistedGraphMutationFailureV2, PersistedGraphTerminalOperationV2, PersistedResponseContextV1,
    RevisionAttemptItemPreconditionsV1, SqliteStoreOptionsV1, SqliteStoreV1, StoreContractV1,
    StoreGraphReadContractV2, StoreGraphStateContractV2, StoreReadContractV1,
    StoreSessionArchiveContractV2, StoreTerminalDispositionContractV2, TerminalReceiptV1,
    TerminalResultV1, WorkerIdV1, WorkspaceBindingV1,
};
use serde_json::{Map, json};

const IDENTITY_PROCEDURE: &str = r#"schema: podway.procedure/v2
id: post-admission-identity
version: "2"
name: Post-admission identity
purpose: Exercise claimed decision and rework jobs after the current session identity changes.
node_definitions:
  choose:
    type: decision
    title: Choose work
    objective: Choose the work branch.
    prompt: Continue to work?
    options:
      - id: continue
        label: Continue
    reason:
      required: true
  work:
    type: action
    title: Work
    intent: Complete the selected work.
graph:
  entry: choose
  nodes:
    - id: choose
      use: choose
      routes:
        continue:
          to: work
          effect: advance
    - id: work
      use: work
      terminal: true
manual_rework:
  allowed_targets:
    - choose
"#;

const TERMINAL_ACTION_PROCEDURE: &str = r#"schema: podway.procedure/v2
id: legacy-start-recovery-current
version: "1"
name: Legacy start recovery current task
purpose: Create a terminal current task for the legacy V17 recovery regression.
node_definitions:
  work:
    type: action
    title: Work
    intent: Complete the current task.
graph:
  entry: work
  nodes:
    - id: work
      use: work
      terminal: true
"#;

type NativeGraphEngine = DaemonExecutionEngineV1<
    Arc<SqliteStoreV1>,
    NativeExecutionIdSourceV1,
    WallUtcExecutionClockV1,
    NativeProcedureProviderV1<SqliteWorkspaceBindingInspectorV1>,
    NativeArtifactVerifierV1<SqliteWorkspaceBindingInspectorV1>,
    NativeWorkspaceRevalidatorV1<SqliteWorkspaceBindingInspectorV1>,
>;

struct SqliteFixture {
    selector: WorktreeSelectorWireV1,
    binding: WorkspaceBindingV1,
    database_path: PathBuf,
    options: SqliteStoreOptionsV1,
    store: Arc<SqliteStoreV1>,
    engine: NativeGraphEngine,
    procedure_digest: podway_core::Sha256Digest,
}

fn native_engine(store: Arc<SqliteStoreV1>, options: &SqliteStoreOptionsV1) -> NativeGraphEngine {
    DaemonExecutionEngineV1::new(
        store,
        NativeExecutionIdSourceV1,
        WallUtcExecutionClockV1,
        NativeProcedureProviderV1::new(SqliteWorkspaceBindingInspectorV1::new(options.clone())),
        NativeArtifactVerifierV1::new(SqliteWorkspaceBindingInspectorV1::new(options.clone())),
        NativeWorkspaceRevalidatorV1::new(SqliteWorkspaceBindingInspectorV1::new(options.clone())),
    )
}

fn sqlite_fixture(root: &std::path::Path) -> SqliteFixture {
    runtime::make_runtime_private(root);
    std::fs::write(root.join("v2drw-identity.yaml"), IDENTITY_PROCEDURE).unwrap();
    let options = SqliteStoreOptionsV1::new(8).unwrap();
    let bootstrap = WorkspaceResolverV1::new(
        NativeGitResolverV1::new(),
        SqliteWorkspaceBindingInspectorV1::new(options.clone()),
    )
    .resolve_bootstrap(support_phase4_workspace::selector(root))
    .unwrap();
    let identity = bootstrap.store_identity().clone();
    let binding = WorkspaceBindingV1::new(identity.clone(), bootstrap.workspace_root().clone());
    let database_path = bootstrap.database_path().to_path_buf();
    let store = Arc::new(
        SqliteStoreV1::open(
            &database_path,
            bootstrap.workspace_root(),
            identity,
            options.clone(),
            UnixMillis::new(1),
        )
        .unwrap(),
    );
    let engine = native_engine(Arc::clone(&store), &options);
    let ParsedProcedure::V2(parsed) =
        parse_procedure_document(IDENTITY_PROCEDURE.as_bytes(), ProcedureDocumentFormat::Yaml)
            .unwrap()
    else {
        unreachable!()
    };
    SqliteFixture {
        selector: runtime::selector(root),
        binding,
        database_path,
        options,
        store,
        engine,
        procedure_digest: validate_procedure_v2(parsed).unwrap().digest().clone(),
    }
}

fn slice_request(
    number: u64,
    command: &str,
    fixture: &SqliteFixture,
    payload: Map<String, serde_json::Value>,
    preconditions: PreconditionsV1,
) -> SliceRequestV1 {
    let request = runtime::request(
        number,
        command,
        &fixture.selector,
        payload,
        "unused-engine-envelope-key",
        preconditions,
    );
    SliceRequestV1::from_envelope(&request.0).unwrap()
}

fn response_context(
    fixture: &SqliteFixture,
    number: u64,
    command: &str,
) -> PersistedResponseContextV1 {
    PersistedResponseContextV1::new(
        format!("00000000-0000-4000-8000-{number:012x}"),
        command,
        fixture.binding.identity().workspace_uuid().clone(),
        fixture
            .binding
            .last_validated_root()
            .to_path_buf()
            .display()
            .to_string(),
        0,
    )
    .unwrap()
}

fn typed_request(
    number: u64,
    command: &str,
    fixture: &SqliteFixture,
    payload: Map<String, serde_json::Value>,
    preconditions: PreconditionsV1,
) -> ProcedureV2MutationRequestV1 {
    let mut payload = payload;
    payload.insert(
        "selector".to_owned(),
        serde_json::to_value(&fixture.selector).unwrap(),
    );
    let envelope = RequestEnvelopeV1::new(RequestEnvelopeInputV1 {
        request_id: RequestIdV1::new(format!("00000000-0000-4000-8000-{number:012x}")).unwrap(),
        client: ClientInfoV1::new("v2drw-epic-identity-test", "1", 1).unwrap(),
        operation: OperationV1::Mutate,
        command: CommandNameV1::new(command).unwrap(),
        workspace: Some(WorkspaceContextV1::new(fixture.selector.display(), None).unwrap()),
        idempotency_key: Some(
            ProtocolIdempotencyKeyV1::new(format!("typed-envelope-{number}")).unwrap(),
        ),
        preconditions,
        options: RequestOptionsV1::new(false, 5_000).unwrap(),
        payload,
    })
    .unwrap();
    let DaemonRequestV1::ProcedureV2Mutation(request) =
        DaemonRequestV1::from_envelope(&envelope).unwrap()
    else {
        panic!("{command} must decode through the typed Procedure v2 route")
    };
    request
}

fn typed_start_request(
    number: u64,
    fixture: &SqliteFixture,
    payload: Map<String, serde_json::Value>,
    preconditions: PreconditionsV1,
) -> ProcedureV2StartRequestV1 {
    let mut payload = payload;
    payload.insert(
        "selector".to_owned(),
        serde_json::to_value(&fixture.selector).unwrap(),
    );
    let envelope = RequestEnvelopeV1::new(RequestEnvelopeInputV1 {
        request_id: RequestIdV1::new(format!("00000000-0000-4000-8000-{number:012x}")).unwrap(),
        client: ClientInfoV1::new("v2drw-legacy-start-recovery-test", "1", 1).unwrap(),
        operation: OperationV1::Mutate,
        command: CommandNameV1::new("session.start_replace").unwrap(),
        workspace: Some(WorkspaceContextV1::new(fixture.selector.display(), None).unwrap()),
        idempotency_key: Some(
            ProtocolIdempotencyKeyV1::new(format!("typed-start-envelope-{number}")).unwrap(),
        ),
        preconditions,
        options: RequestOptionsV1::new(false, 5_000).unwrap(),
        payload,
    })
    .unwrap();
    let DaemonRequestV1::ProcedureV2Start(request) =
        DaemonRequestV1::from_envelope(&envelope).unwrap()
    else {
        panic!("session.start_replace must decode through the typed Procedure v2 start route")
    };
    request
}

fn start_graph(fixture: &SqliteFixture, number: u64, key: &str) -> SessionId {
    let request = slice_request(
        number,
        "session.start",
        fixture,
        json!({
            "procedure": "v2drw-identity.yaml",
            "expected_procedure_digest": fixture.procedure_digest,
            "task_title": "Post-admission session identity regression"
        })
        .as_object()
        .unwrap()
        .clone(),
        PreconditionsV1::default(),
    );
    assert!(matches!(
        fixture
            .engine
            .admit_procedure_v2_start_for_workspace_with_response_context(
                &fixture.binding,
                &request,
                IdempotencyKeyV1::new(key).unwrap(),
                Some(response_context(fixture, number, "session.start")),
            )
            .unwrap(),
        Some(AdmitOutcomeV1::New(_))
    ));
    let terminal = fixture
        .engine
        .execute_next_with_graph_v2(
            &fixture.binding,
            WorkerIdV1::new(format!("{key}-worker")).unwrap(),
        )
        .unwrap()
        .unwrap();
    assert!(matches!(terminal.result(), TerminalResultV1::Success(_)));
    fixture
        .store
        .read_graph_session_v2(fixture.binding.identity())
        .unwrap()
        .unwrap()
        .trace()
        .session_id()
        .clone()
}

fn graph_preconditions(fixture: &SqliteFixture) -> PreconditionsV1 {
    let state = fixture
        .store
        .read_graph_session_v2(fixture.binding.identity())
        .unwrap()
        .unwrap();
    PreconditionsV1::new(
        Some(state.trace().session_id().clone()),
        Some(state.trace().revision()),
        Some(state.trace().active_attempt().unwrap().attempt_id().clone()),
        None,
        None,
        None,
    )
    .unwrap()
}

fn admit_typed(
    fixture: &SqliteFixture,
    request: &ProcedureV2MutationRequestV1,
    key: &str,
    number: u64,
    command: &str,
) {
    assert!(matches!(
        fixture
            .engine
            .admit_procedure_v2_typed_mutation_for_workspace_with_response_context(
                &fixture.binding,
                request,
                IdempotencyKeyV1::new(key).unwrap(),
                Some(response_context(fixture, number, command)),
            )
            .unwrap(),
        Some(AdmitOutcomeV1::New(_))
    ));
}

fn admit_slice_mutation(
    fixture: &SqliteFixture,
    request: &SliceRequestV1,
    key: &str,
    number: u64,
    command: &str,
) {
    assert!(matches!(
        fixture
            .engine
            .admit_procedure_v2_mutation_for_workspace_with_response_context(
                &fixture.binding,
                request,
                IdempotencyKeyV1::new(key).unwrap(),
                Some(response_context(fixture, number, command)),
            )
            .unwrap(),
        Some(AdmitOutcomeV1::New(_))
    ));
}

fn claim_and_clear_graph(
    fixture: &SqliteFixture,
    worker: &str,
) -> podway_store::GraphSessionStateV2 {
    let state = fixture
        .store
        .read_graph_session_v2(fixture.binding.identity())
        .unwrap()
        .unwrap();
    let claimed = fixture
        .store
        .claim_next(
            fixture.binding.identity(),
            WorkerIdV1::new(worker).unwrap(),
            WallUtcExecutionClockV1.now(),
        )
        .unwrap()
        .expect("the just-admitted mutation must be claimed before graph drift");
    assert_eq!(claimed.claim().identity(), fixture.binding.identity());
    fixture
        .store
        .clear_graph_session_v2(
            fixture.binding.identity(),
            state.workspace_revision(),
            state.trace().revision(),
        )
        .unwrap();
    state
}

fn reopen_after_claimed_recovery(fixture: SqliteFixture) -> SqliteFixture {
    let SqliteFixture {
        selector,
        binding,
        database_path,
        options,
        store,
        engine,
        procedure_digest,
    } = fixture;
    drop(engine);
    drop(store);
    let reopened = Arc::new(
        SqliteStoreV1::open(
            &database_path,
            binding.last_validated_root(),
            binding.identity().clone(),
            options.clone(),
            WallUtcExecutionClockV1.now(),
        )
        .unwrap(),
    );
    assert_eq!(
        reopened.startup_recovery_report().requeued_job_count(),
        1,
        "startup recovery must requeue the deliberately claimed mutation"
    );
    let engine = native_engine(Arc::clone(&reopened), &options);
    SqliteFixture {
        selector,
        binding,
        database_path,
        options,
        store: reopened,
        engine,
        procedure_digest,
    }
}

fn assert_identity_failure(
    terminal: &TerminalReceiptV1,
    expected: &SessionId,
    actual: Option<&SessionId>,
) {
    assert_eq!(
        terminal.result(),
        &TerminalResultV1::Failure(DomainError::SessionIdentityMismatch {
            expected: expected.clone(),
            actual: actual.cloned(),
        })
    );
}

fn assert_cold_replay(
    fixture: SqliteFixture,
    request: &ProcedureV2MutationRequestV1,
    key: &str,
    number: u64,
    command: &str,
    terminal: &TerminalReceiptV1,
) {
    let replay_context = response_context(&fixture, number, command);
    let expected = fixture
        .store
        .read_job(fixture.binding.identity(), terminal.job().job_id())
        .unwrap()
        .unwrap()
        .terminal_receipt()
        .unwrap()
        .clone();
    let binding = fixture.binding.clone();
    let database_path = fixture.database_path.clone();
    let options = fixture.options.clone();
    let identity = binding.identity().clone();
    drop(fixture.engine);
    drop(fixture.store);
    let reopened = Arc::new(
        SqliteStoreV1::open(
            &database_path,
            binding.last_validated_root(),
            identity,
            options.clone(),
            WallUtcExecutionClockV1.now(),
        )
        .unwrap(),
    );
    let restarted = native_engine(reopened, &options);
    assert_eq!(
        restarted
            .admit_procedure_v2_typed_mutation_for_workspace_with_response_context(
                &binding,
                request,
                IdempotencyKeyV1::new(key).unwrap(),
                Some(replay_context),
            )
            .unwrap(),
        Some(AdmitOutcomeV1::Existing(
            JobReceiptOrTerminalV1::TerminalReceipt(expected)
        ))
    );
}

fn assert_cold_replay_slice(
    fixture: SqliteFixture,
    request: &SliceRequestV1,
    key: &str,
    number: u64,
    command: &str,
    terminal: &TerminalReceiptV1,
) {
    let replay_context = response_context(&fixture, number, command);
    let expected = fixture
        .store
        .read_job(fixture.binding.identity(), terminal.job().job_id())
        .unwrap()
        .unwrap()
        .terminal_receipt()
        .unwrap()
        .clone();
    let binding = fixture.binding.clone();
    let database_path = fixture.database_path.clone();
    let options = fixture.options.clone();
    let identity = binding.identity().clone();
    drop(fixture.engine);
    drop(fixture.store);
    let reopened = Arc::new(
        SqliteStoreV1::open(
            &database_path,
            binding.last_validated_root(),
            identity,
            options.clone(),
            WallUtcExecutionClockV1.now(),
        )
        .unwrap(),
    );
    let restarted = native_engine(reopened, &options);
    assert_eq!(
        restarted
            .admit_procedure_v2_mutation_for_workspace_with_response_context(
                &binding,
                request,
                IdempotencyKeyV1::new(key).unwrap(),
                Some(replay_context),
            )
            .unwrap(),
        Some(AdmitOutcomeV1::Existing(
            JobReceiptOrTerminalV1::TerminalReceipt(expected)
        ))
    );
}

#[test]
fn legacy_v17_eligible_start_recovery_terminalizes_ineligible_job_once() {
    let workspace = support_phase4_workspace::git_worktrees();
    let _terminal_sealer = runtime::dispatcher(
        Arc::new(runtime::manager(workspace.temporary_path())),
        "legacy-v17-terminal-sealer",
    );
    let mut fixture = sqlite_fixture(workspace.main());
    let current_session_id = SessionId::new("00000000-0000-4000-8000-000000109001").unwrap();
    let current_snapshot = workspace_procedure_snapshot_from_bytes_v2(
        "legacy-start-recovery-current.yaml",
        TERMINAL_ACTION_PROCEDURE.as_bytes(),
        podway_core::ProcedureSnapshotId::new("00000000-0000-4000-8000-000000109002").unwrap(),
        UnixMillis::new(10),
    )
    .unwrap();
    let prepared = graph_prepared_session_state_from_procedure_v2_snapshot(
        current_snapshot,
        "Legacy V17 current task",
        current_session_id.clone(),
        UnixMillis::new(10),
    )
    .unwrap();
    fixture
        .store
        .create_graph_session_v2(fixture.binding.identity(), prepared.clone())
        .unwrap();
    let attempt_id = AttemptId::new("00000000-0000-4000-8000-000000109003").unwrap();
    let running = prepared
        .begin_v2(
            Revision::ZERO,
            attempt_id.clone(),
            None,
            UnixMillis::new(11),
        )
        .unwrap()
        .into_state();
    fixture
        .store
        .replace_graph_session_v2(
            fixture.binding.identity(),
            prepared.workspace_revision(),
            prepared.trace().revision(),
            running.clone(),
        )
        .unwrap();
    let completed = running
        .complete_active_action_v2(Revision::new(1), &attempt_id, None, UnixMillis::new(12))
        .unwrap()
        .into_state();
    fixture
        .store
        .replace_graph_session_v2(
            fixture.binding.identity(),
            running.workspace_revision(),
            running.trace().revision(),
            completed.clone(),
        )
        .unwrap();
    fixture
        .store
        .record_terminal_disposition_v2(
            fixture.binding.identity(),
            TerminalDispositionV2::not_required(
                current_session_id.clone(),
                Revision::new(2),
                "Permit the V19 template admission before removing the disposition.",
                None,
                UnixMillis::new(13),
            )
            .unwrap(),
        )
        .unwrap();

    let replace = typed_start_request(
        109_010,
        &fixture,
        json!({
            "procedure": "v2drw-identity.yaml",
            "expected_procedure_digest": fixture.procedure_digest,
            "task_title": "Replacement admitted by legacy V17",
            "replace_eligible": true
        })
        .as_object()
        .unwrap()
        .clone(),
        PreconditionsV1::new(
            Some(current_session_id.clone()),
            Some(Revision::new(2)),
            None,
            None,
            None,
            None,
        )
        .unwrap(),
    );
    let template_key = IdempotencyKeyV1::new("legacy-v19-template-start").unwrap();
    let admitted = fixture
        .engine
        .admit_procedure_v2_typed_start_for_workspace_with_response_context(
            &fixture.binding,
            &replace,
            template_key,
            Some(response_context(&fixture, 109_010, "session.start_replace")),
        )
        .unwrap()
        .unwrap();
    let AdmitOutcomeV1::New(job) = admitted else {
        panic!("the V19 template start must be newly admitted")
    };
    let persisted = fixture
        .store
        .read_job(fixture.binding.identity(), job.job_id())
        .unwrap()
        .unwrap();
    let mut execution: serde_json::Value =
        serde_json::from_str(persisted.execution().canonical_execution().as_str()).unwrap();
    assert_eq!(execution["execution_version"], 19);
    execution["execution_version"] = json!(17);
    let legacy_execution = CanonicalExecutionJsonV1::new(execution.to_string()).unwrap();
    assert!(matches!(
        fixture
            .engine
            .execute_next_with_graph_v2(
                &fixture.binding,
                WorkerIdV1::new("legacy-v19-template-worker").unwrap(),
            )
            .unwrap()
            .unwrap()
            .result(),
        TerminalResultV1::Success(_)
    ));
    let template_state = fixture
        .store
        .read_graph_session_v2(fixture.binding.identity())
        .unwrap()
        .unwrap();
    fixture
        .store
        .purge_archived_session_v2(
            fixture.binding.identity(),
            &current_session_id,
            Revision::new(2),
        )
        .unwrap();
    fixture
        .store
        .clear_graph_session_v2(
            fixture.binding.identity(),
            template_state.workspace_revision(),
            template_state.trace().revision(),
        )
        .unwrap();
    fixture
        .store
        .create_graph_session_v2(fixture.binding.identity(), completed.clone())
        .unwrap();

    let key = IdempotencyKeyV1::new("legacy-v17-eligible-start").unwrap();
    let legacy_job_id = JobIdV1::new("00000000-0000-4000-8000-000000109004").unwrap();
    let legacy = AdmitRequestV1::new_with_canonical_execution(
        CommandV1::SessionStartReplace,
        key.clone(),
        legacy_job_id.clone(),
        RevisionAttemptItemPreconditionsV1::new(Some(Revision::new(2)), None, None, None).unwrap(),
        job.request_digest().clone(),
        UnixMillis::new(20),
        legacy_execution,
    )
    .with_procedure_v2_execution()
    .with_session_identity(AdmissionSessionIdentityV1::Exact(
        current_session_id.clone(),
    ))
    .with_response_context(
        response_context(&fixture, 109_011, "session.start_replace")
            .with_frozen_public_terminal_envelope(),
    );
    assert!(matches!(
        fixture.store.admit(fixture.binding.identity(), legacy).unwrap(),
        AdmitOutcomeV1::New(receipt) if receipt.job_id() == &legacy_job_id
    ));

    fixture
        .store
        .claim_next(
            fixture.binding.identity(),
            WorkerIdV1::new("legacy-v17-first-claim").unwrap(),
            UnixMillis::new(20),
        )
        .unwrap()
        .expect("the admitted legacy start must be claimable");

    fixture = reopen_after_claimed_recovery(fixture);
    let terminal = fixture
        .engine
        .execute_next_with_graph_v2(
            &fixture.binding,
            WorkerIdV1::new("legacy-v17-recovery-worker").unwrap(),
        )
        .unwrap()
        .unwrap();
    assert!(matches!(terminal.result(), TerminalResultV1::Failure(_)));
    let terminal_job = fixture
        .store
        .read_job(fixture.binding.identity(), terminal.job().job_id())
        .unwrap()
        .unwrap();
    assert_eq!(terminal_job.state(), JobStateV1::Failed);
    let terminal_receipt = terminal_job.terminal_receipt().unwrap();
    assert!(matches!(
        terminal_receipt
            .graph_session_projection()
            .unwrap()
            .operation(),
        Some(PersistedGraphTerminalOperationV2::Failure {
            error: PersistedGraphMutationFailureV2::SessionResetNotEligible {
                lifecycle,
                current_terminal_disposition: false,
            },
        }) if lifecycle == "completed"
    ));
    assert_eq!(
        terminal_receipt.public_terminal_envelope().unwrap()["code"],
        "SESSION_RESET_NOT_ELIGIBLE"
    );
    assert_eq!(
        fixture
            .store
            .read_graph_session_v2(fixture.binding.identity())
            .unwrap(),
        Some(completed.clone()),
        "legacy start recovery must not change the undisposed completed session"
    );
    let view = fixture
        .store
        .read_graph_workspace_view_v2(fixture.binding.identity())
        .unwrap();
    assert_eq!(view.queued_job_count(), 0);
    assert_eq!(view.running_job_id(), None);
    assert_eq!(
        fixture
            .engine
            .admit_procedure_v2_typed_start_for_workspace_with_response_context(
                &fixture.binding,
                &replace,
                key,
                Some(
                    response_context(&fixture, 109_011, "session.start_replace")
                        .with_frozen_public_terminal_envelope(),
                ),
            )
            .unwrap(),
        Some(AdmitOutcomeV1::Existing(
            JobReceiptOrTerminalV1::TerminalReceipt(terminal_receipt.clone())
        ))
    );

    let SqliteFixture {
        selector,
        binding,
        database_path,
        options,
        store,
        engine,
        procedure_digest,
    } = fixture;
    drop(engine);
    drop(store);
    let reopened = Arc::new(
        SqliteStoreV1::open(
            &database_path,
            binding.last_validated_root(),
            binding.identity().clone(),
            options.clone(),
            UnixMillis::new(50),
        )
        .unwrap(),
    );
    assert_eq!(reopened.startup_recovery_report().requeued_job_count(), 0);
    assert_eq!(
        reopened.read_graph_session_v2(binding.identity()).unwrap(),
        Some(completed)
    );
    let final_view = reopened
        .read_graph_workspace_view_v2(binding.identity())
        .unwrap();
    assert_eq!(final_view.queued_job_count(), 0);
    assert_eq!(final_view.running_job_id(), None);
    drop((selector, procedure_digest));
}

#[test]
fn v2drw_claimed_decide_after_session_clear_terminalizes_mismatch_and_replays() {
    let workspace = support_phase4_workspace::git_worktrees();
    let mut fixture = sqlite_fixture(workspace.main());
    let old_session_id = start_graph(&fixture, 110_001, "identity-decide-start");
    let decide = typed_request(
        110_010,
        "session.decide",
        &fixture,
        json!({"option_id": "continue", "reason": "Execute after claimed recovery."})
            .as_object()
            .unwrap()
            .clone(),
        graph_preconditions(&fixture),
    );
    admit_typed(
        &fixture,
        &decide,
        "identity-decide-after-clear",
        110_010,
        "session.decide",
    );
    let claimed_state = claim_and_clear_graph(&fixture, "identity-decide-claimed");
    assert_eq!(claimed_state.trace().session_id(), &old_session_id);
    assert!(
        fixture
            .store
            .read_graph_session_v2(fixture.binding.identity())
            .unwrap()
            .is_none()
    );
    fixture = reopen_after_claimed_recovery(fixture);
    let terminal = fixture
        .engine
        .execute_next_with_graph_v2(
            &fixture.binding,
            WorkerIdV1::new("identity-decide-worker").unwrap(),
        )
        .unwrap()
        .unwrap();
    assert_identity_failure(&terminal, &old_session_id, None);
    assert!(
        fixture
            .store
            .read_graph_session_v2(fixture.binding.identity())
            .unwrap()
            .is_none(),
        "the claimed decision must not resurrect a cleared graph"
    );
    assert_cold_replay(
        fixture,
        &decide,
        "identity-decide-after-clear",
        110_099,
        "session.decide",
        &terminal,
    );
}

#[test]
fn v2drw_claimed_rework_after_session_replacement_terminalizes_mismatch_and_replays() {
    let workspace = support_phase4_workspace::git_worktrees();
    let mut fixture = sqlite_fixture(workspace.main());
    let old_session_id = start_graph(&fixture, 111_001, "identity-rework-start");
    let advance = typed_request(
        111_010,
        "session.decide",
        &fixture,
        json!({"option_id": "continue", "reason": "Reach work before rework."})
            .as_object()
            .unwrap()
            .clone(),
        graph_preconditions(&fixture),
    );
    admit_typed(
        &fixture,
        &advance,
        "identity-rework-advance",
        111_010,
        "session.decide",
    );
    let advanced = fixture
        .engine
        .execute_next_with_graph_v2(
            &fixture.binding,
            WorkerIdV1::new("identity-rework-advance-worker").unwrap(),
        )
        .unwrap()
        .unwrap();
    assert!(matches!(advanced.result(), TerminalResultV1::Success(_)));

    let rework = typed_request(
        111_020,
        "session.rework",
        &fixture,
        json!({
            "target_graph_node_id": "choose",
            "reason": "Execute after claimed replacement recovery."
        })
        .as_object()
        .unwrap()
        .clone(),
        graph_preconditions(&fixture),
    );
    admit_typed(
        &fixture,
        &rework,
        "identity-rework-after-replacement",
        111_020,
        "session.rework",
    );
    let claimed_state = claim_and_clear_graph(&fixture, "identity-rework-claimed");
    let replacement = graph_session_state_from_procedure_v2_snapshot(
        claimed_state.snapshot().clone(),
        "Replacement graph after claimed rework",
        SessionId::new("00000000-0000-4000-8000-000000111099").unwrap(),
        AttemptId::new("00000000-0000-4000-8000-000000111100").unwrap(),
        WallUtcExecutionClockV1.now(),
    )
    .unwrap();
    fixture
        .store
        .create_graph_session_v2(fixture.binding.identity(), replacement.clone())
        .unwrap();
    let replacement_session_id = replacement.trace().session_id().clone();
    assert_ne!(replacement_session_id, old_session_id);
    fixture = reopen_after_claimed_recovery(fixture);
    let terminal = fixture
        .engine
        .execute_next_with_graph_v2(
            &fixture.binding,
            WorkerIdV1::new("identity-rework-worker").unwrap(),
        )
        .unwrap()
        .unwrap();
    assert_identity_failure(&terminal, &old_session_id, Some(&replacement_session_id));
    assert_eq!(
        fixture
            .store
            .read_graph_session_v2(fixture.binding.identity())
            .unwrap(),
        Some(replacement),
        "the stale rework must not change the replacement graph"
    );
    assert_cold_replay(
        fixture,
        &rework,
        "identity-rework-after-replacement",
        111_099,
        "session.rework",
        &terminal,
    );
}

#[test]
fn v2run_claimed_complete_after_session_clear_terminalizes_mismatch_and_replays() {
    let workspace = support_phase4_workspace::git_worktrees();
    let mut fixture = sqlite_fixture(workspace.main());
    let old_session_id = start_graph(&fixture, 112_001, "identity-complete-start");
    let advance = typed_request(
        112_010,
        "session.decide",
        &fixture,
        json!({"option_id": "continue", "reason": "Reach work before completion."})
            .as_object()
            .unwrap()
            .clone(),
        graph_preconditions(&fixture),
    );
    admit_typed(
        &fixture,
        &advance,
        "identity-complete-advance",
        112_010,
        "session.decide",
    );
    assert!(matches!(
        fixture
            .engine
            .execute_next_with_graph_v2(
                &fixture.binding,
                WorkerIdV1::new("identity-complete-advance-worker").unwrap(),
            )
            .unwrap()
            .unwrap()
            .result(),
        TerminalResultV1::Success(_)
    ));

    let complete = slice_request(
        112_020,
        "session.complete",
        &fixture,
        Map::new(),
        graph_preconditions(&fixture),
    );
    admit_slice_mutation(
        &fixture,
        &complete,
        "identity-complete-after-clear",
        112_020,
        "session.complete",
    );
    let claimed_state = claim_and_clear_graph(&fixture, "identity-complete-claimed");
    assert_eq!(claimed_state.trace().session_id(), &old_session_id);
    fixture = reopen_after_claimed_recovery(fixture);
    let terminal = fixture
        .engine
        .execute_next_with_graph_v2(
            &fixture.binding,
            WorkerIdV1::new("identity-complete-worker").unwrap(),
        )
        .unwrap()
        .unwrap();
    assert_identity_failure(&terminal, &old_session_id, None);
    assert!(
        fixture
            .store
            .read_graph_session_v2(fixture.binding.identity())
            .unwrap()
            .is_none(),
        "the claimed completion must not resurrect a cleared graph"
    );
    assert_cold_replay_slice(
        fixture,
        &complete,
        "identity-complete-after-clear",
        112_099,
        "session.complete",
        &terminal,
    );
}
