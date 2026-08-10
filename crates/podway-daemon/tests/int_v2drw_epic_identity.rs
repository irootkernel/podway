//! Real-SQLite regressions for post-admission Procedure v2 session identity drift.

use super::{int_v2run003_runtime as runtime, support_phase4_workspace};

use std::{path::PathBuf, sync::Arc};

use podway_config::{
    ParsedProcedure, ProcedureDocumentFormat, parse_procedure_document, validate_procedure_v2,
};
use podway_core::{AttemptId, DomainError, SessionId, UnixMillis};
use podway_daemon::{
    execution::{
        DaemonExecutionEngineV1, ExecutionClockV1, graph_session_state_from_procedure_v2_snapshot,
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
    PreconditionsV1, ProcedureV2MutationRequestV1, RequestEnvelopeInputV1, RequestEnvelopeV1,
    RequestIdV1, RequestOptionsV1, SliceRequestV1, WorkspaceContextV1, WorktreeSelectorWireV1,
};
use podway_store::{
    AdmitOutcomeV1, IdempotencyKeyV1, JobReceiptOrTerminalV1, PersistedResponseContextV1,
    SqliteStoreOptionsV1, SqliteStoreV1, StoreContractV1, StoreGraphStateContractV2,
    StoreReadContractV1, TerminalReceiptV1, TerminalResultV1, WorkerIdV1, WorkspaceBindingV1,
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
