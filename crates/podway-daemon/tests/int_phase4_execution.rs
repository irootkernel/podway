use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

use podway_config::{
    ProcedureFormatV1, ProcedureSourceLabel, ProcedureWarningPolicyV1, parse_procedure_v1,
};
use podway_core::{
    ArtifactValueV1, AttemptId, BlockerId, DomainCommand, DomainError, JobId,
    LocalArtifactVerificationV1, ProcedureSnapshotId, ProcedureSnapshotV1, Revision,
    SessionAggregateV1, SessionId, Sha256Digest, UnixMillis, WorkspaceId, WorkspaceState,
};
use podway_daemon::execution::{
    ArtifactVerifierV1, DaemonExecutionEngineV1, EmbeddedPresetProcedureProviderV1,
    ExecutionBoundaryErrorV1, ExecutionClockV1, ExecutionErrorV1, ExecutionIdSourceV1,
    ProcedureProviderV1, WorkspaceRevalidatorV1,
};
use podway_protocol::{
    ClientInfoV1, CommandNameV1, IdempotencyKeyV1 as ProtocolIdempotencyKeyV1, OperationV1,
    PreconditionsV1, RequestEnvelopeInputV1, RequestEnvelopeV1, RequestIdV1, RequestOptionsV1,
    SliceRequestV1, WorkspaceContextV1, WorktreeSelectorWireV1,
};
use podway_store::{
    AdmissionSessionIdentityV1, AdmitOutcomeV1, AdmitRequestV1, CancelOutcomeV1,
    CanonicalExecutionJsonV1, ClaimTokenV1, ClaimedExecutionV1, ClaimedJobV1,
    DurableWorktreeIdentityV1, IdempotencyKeyV1, JobReceiptOrTerminalV1, JobReceiptV1,
    PersistedSessionMutationV1, PersistedTerminalReceiptV1, RevisionAttemptItemPreconditionsV1,
    StateTransitionV1, StoreContractV1, StoreErrorV1, StoreIdempotencyReadContractV1,
    TerminalReceiptV1, TerminalResultV1, WorkerIdV1, WorkspaceBindingV1, WorkspaceViewV1,
};
use serde_json::{Map, Value, json};

const WORKSPACE_ID: &str = "00000000-0000-4000-8000-000000000001";
const DIGEST: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const PROCEDURE_YAML: &[u8] = br#"
schema: podway.procedure/v1
id: execution-test
version: "1"
name: Execution Test
stages:
  - id: first
    title: First
    skip:
      allowed: true
      reason_required: true
    items:
      - type: confirm
        id: confirm
        prompt: Confirm
        required: true
      - type: text
        id: notes
        prompt: Notes
        required: false
      - type: list
        id: entries
        prompt: Entries
        required: false
      - type: artifact
        id: proof
        prompt: Proof
        required: true
  - id: second
    title: Second
    items:
      - type: confirm
        id: finish
        prompt: Finish
        required: true
rework:
  allow_return_to: any_previous
"#;
const LEGACY_V1_SESSION_START_DOCUMENT: &str = "{\"command\":\"session.start\",\"execution_version\":1,\"payload\":{\"preset\":\"sw-dev\",\"task_title\":\"Legacy V1\"},\"preconditions\":{},\"selector\":{\"display\":\"/worktree\",\"expected_uuid\":\"00000000-0000-4000-8000-000000000001\",\"path_bytes_base64url\":\"L3dvcmt0cmVl\",\"version\":1},\"workspace_id\":\"00000000-0000-4000-8000-000000000001\"}";
const LEGACY_V2_SESSION_START_DOCUMENT: &str = "{\"command\":\"session.start\",\"execution_version\":2,\"payload\":{\"preset\":\"sw-dev\",\"task_title\":\"Legacy V2\"},\"preconditions\":{},\"selector\":{\"display\":\"/worktree\",\"expected_uuid\":\"00000000-0000-4000-8000-000000000001\",\"path_bytes_base64url\":\"L3dvcmt0cmVl\",\"version\":1},\"workspace_id\":\"00000000-0000-4000-8000-000000000001\"}";

#[derive(Clone)]
struct FixtureIds(Arc<Mutex<u64>>);

impl FixtureIds {
    fn new() -> Self {
        Self(Arc::new(Mutex::new(10)))
    }

    fn uuid(&self) -> String {
        let mut next = self.0.lock().unwrap();
        *next += 1;
        format!("00000000-0000-4000-8000-{next:012}")
    }
}

impl ExecutionIdSourceV1 for FixtureIds {
    fn next_job_id(&self) -> JobId {
        JobId::new(self.uuid()).unwrap()
    }

    fn next_session_id(&self) -> SessionId {
        SessionId::new(self.uuid()).unwrap()
    }

    fn next_attempt_id(&self) -> AttemptId {
        AttemptId::new(self.uuid()).unwrap()
    }

    fn next_blocker_id(&self) -> BlockerId {
        BlockerId::new(self.uuid()).unwrap()
    }

    fn next_procedure_snapshot_id(&self) -> ProcedureSnapshotId {
        ProcedureSnapshotId::new(self.uuid()).unwrap()
    }
}
#[derive(Clone)]
struct SpyIds {
    ids: FixtureIds,
    calls: Arc<Mutex<Vec<&'static str>>>,
}

impl SpyIds {
    fn new() -> Self {
        Self {
            ids: FixtureIds::new(),
            calls: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn calls(&self) -> Vec<&'static str> {
        self.calls.lock().unwrap().clone()
    }

    fn record(&self, operation: &'static str) {
        self.calls.lock().unwrap().push(operation);
    }
}

impl ExecutionIdSourceV1 for SpyIds {
    fn next_job_id(&self) -> JobId {
        self.record("job");
        self.ids.next_job_id()
    }

    fn next_session_id(&self) -> SessionId {
        self.record("session");
        self.ids.next_session_id()
    }

    fn next_attempt_id(&self) -> AttemptId {
        self.record("attempt");
        self.ids.next_attempt_id()
    }

    fn next_blocker_id(&self) -> BlockerId {
        self.record("blocker");
        self.ids.next_blocker_id()
    }

    fn next_procedure_snapshot_id(&self) -> ProcedureSnapshotId {
        self.record("snapshot");
        self.ids.next_procedure_snapshot_id()
    }
}

#[derive(Clone)]
struct FixtureClock(Arc<Mutex<u64>>);

impl FixtureClock {
    fn new() -> Self {
        Self(Arc::new(Mutex::new(100)))
    }
}

impl ExecutionClockV1 for FixtureClock {
    fn now(&self) -> UnixMillis {
        let mut now = self.0.lock().unwrap();
        *now += 1;
        UnixMillis::new(*now)
    }
}
#[derive(Clone)]
struct SpyClock {
    calls: Arc<AtomicUsize>,
}

impl SpyClock {
    fn new() -> Self {
        Self {
            calls: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl ExecutionClockV1 for SpyClock {
    fn now(&self) -> UnixMillis {
        self.calls.fetch_add(1, Ordering::SeqCst);
        UnixMillis::new(100)
    }
}

#[derive(Clone, Copy)]
struct FixtureProcedures;

impl ProcedureProviderV1 for FixtureProcedures {
    fn load_preset_snapshot(
        &self,
        _preset: &str,
        snapshot_id: ProcedureSnapshotId,
        created_at: UnixMillis,
    ) -> Result<ProcedureSnapshotV1, ExecutionBoundaryErrorV1> {
        parse_procedure_v1(PROCEDURE_YAML, ProcedureFormatV1::Yaml)
            .and_then(|procedure| {
                procedure.into_snapshot_v1(
                    snapshot_id,
                    ProcedureSourceLabel::preset("execution-test")?,
                    created_at,
                    ProcedureWarningPolicyV1::Accept,
                )
            })
            .map_err(|_| {
                ExecutionBoundaryErrorV1::domain(DomainError::InvalidState {
                    reason: "fixture procedure admission failed",
                })
            })
    }
    fn load_workspace_procedure_snapshot(
        &self,
        _workspace: &WorkspaceBindingV1,
        _procedure: &str,
        snapshot_id: ProcedureSnapshotId,
        created_at: UnixMillis,
    ) -> Result<ProcedureSnapshotV1, ExecutionBoundaryErrorV1> {
        self.load_preset_snapshot("execution-test", snapshot_id, created_at)
    }
}
#[derive(Clone)]
struct SpyProcedures {
    calls: Arc<AtomicUsize>,
}

impl SpyProcedures {
    fn new() -> Self {
        Self {
            calls: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }
}

impl ProcedureProviderV1 for SpyProcedures {
    fn load_preset_snapshot(
        &self,
        preset: &str,
        snapshot_id: ProcedureSnapshotId,
        created_at: UnixMillis,
    ) -> Result<ProcedureSnapshotV1, ExecutionBoundaryErrorV1> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        FixtureProcedures.load_preset_snapshot(preset, snapshot_id, created_at)
    }
    fn load_workspace_procedure_snapshot(
        &self,
        workspace: &WorkspaceBindingV1,
        procedure: &str,
        snapshot_id: ProcedureSnapshotId,
        created_at: UnixMillis,
    ) -> Result<ProcedureSnapshotV1, ExecutionBoundaryErrorV1> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        FixtureProcedures.load_workspace_procedure_snapshot(
            workspace,
            procedure,
            snapshot_id,
            created_at,
        )
    }
}

#[derive(Clone, Copy)]
struct FixtureArtifacts;

impl ArtifactVerifierV1 for FixtureArtifacts {
    fn hash_local_artifact(
        &self,
        _workspace: &WorkspaceBindingV1,
        path: &str,
        requested_media_type: Option<&str>,
    ) -> Result<ArtifactValueV1, ExecutionBoundaryErrorV1> {
        ArtifactValueV1::local_path(
            path,
            Sha256Digest::new(DIGEST).unwrap(),
            7,
            requested_media_type.unwrap_or("application/octet-stream"),
        )
        .map_err(ExecutionBoundaryErrorV1::domain)
    }

    fn revalidate_local_artifact(
        &self,
        _workspace: &WorkspaceBindingV1,
        item_id: &podway_core::ItemId,
        artifact: &ArtifactValueV1,
    ) -> Result<LocalArtifactVerificationV1, ExecutionBoundaryErrorV1> {
        Ok(LocalArtifactVerificationV1 {
            item_id: item_id.clone(),
            location: artifact.location().to_owned(),
            digest: artifact.digest().clone(),
            size_bytes: artifact.size_bytes(),
        })
    }
}

#[derive(Clone)]
struct FixtureWorkspaces {
    selector_binding: WorkspaceBindingV1,
    manager_binding: WorkspaceBindingV1,
    selectors_available: Arc<AtomicBool>,
    manager_binding_available: Arc<AtomicBool>,
    selector_revalidations: Arc<AtomicUsize>,
    binding_revalidations: Arc<AtomicUsize>,
    selector_identity_mismatch: Arc<Mutex<Option<(WorkspaceId, WorkspaceId)>>>,
}

impl FixtureWorkspaces {
    fn stable(binding: WorkspaceBindingV1) -> Self {
        Self {
            selector_binding: binding.clone(),
            manager_binding: binding,
            selectors_available: Arc::new(AtomicBool::new(true)),
            manager_binding_available: Arc::new(AtomicBool::new(true)),
            selector_revalidations: Arc::new(AtomicUsize::new(0)),
            binding_revalidations: Arc::new(AtomicUsize::new(0)),
            selector_identity_mismatch: Arc::new(Mutex::new(None)),
        }
    }

    fn moved(selector_binding: WorkspaceBindingV1, manager_binding: WorkspaceBindingV1) -> Self {
        Self {
            selector_binding,
            manager_binding,
            selectors_available: Arc::new(AtomicBool::new(true)),
            manager_binding_available: Arc::new(AtomicBool::new(true)),
            selector_revalidations: Arc::new(AtomicUsize::new(0)),
            binding_revalidations: Arc::new(AtomicUsize::new(0)),
            selector_identity_mismatch: Arc::new(Mutex::new(None)),
        }
    }

    fn reject_stale_selectors(&self) {
        self.selectors_available.store(false, Ordering::SeqCst);
    }
    fn reject_manager_binding(&self) {
        self.manager_binding_available
            .store(false, Ordering::SeqCst);
    }

    fn mismatch_selector_identity(&self, expected: WorkspaceId, actual: WorkspaceId) {
        *self.selector_identity_mismatch.lock().unwrap() = Some((expected, actual));
    }
}

impl WorkspaceRevalidatorV1 for FixtureWorkspaces {
    fn revalidate(
        &self,
        _selector: &WorktreeSelectorWireV1,
    ) -> Result<WorkspaceBindingV1, ExecutionBoundaryErrorV1> {
        self.selector_revalidations.fetch_add(1, Ordering::SeqCst);
        if let Some((expected, actual)) = self.selector_identity_mismatch.lock().unwrap().clone() {
            return Err(ExecutionBoundaryErrorV1::workspace_identity_mismatch(
                expected, actual,
            ));
        }
        if !self.selectors_available.load(Ordering::SeqCst) {
            return Err(ExecutionBoundaryErrorV1::domain(
                DomainError::InvalidState {
                    reason: "stale selector no longer resolves",
                },
            ));
        }
        Ok(self.selector_binding.clone())
    }

    fn revalidate_binding(
        &self,
        binding: &WorkspaceBindingV1,
    ) -> Result<WorkspaceBindingV1, ExecutionBoundaryErrorV1> {
        self.binding_revalidations.fetch_add(1, Ordering::SeqCst);
        if !self.manager_binding_available.load(Ordering::SeqCst) {
            return Err(ExecutionBoundaryErrorV1::domain(
                DomainError::InvalidState {
                    reason: "manager binding was deleted",
                },
            ));
        }
        if binding != &self.manager_binding {
            return Err(ExecutionBoundaryErrorV1::domain(
                DomainError::InvalidState {
                    reason: "manager binding was replaced",
                },
            ));
        }
        Ok(self.manager_binding.clone())
    }
}

#[derive(Clone)]
struct RecordingStore {
    identity: DurableWorktreeIdentityV1,
    state: Arc<Mutex<RecordingState>>,
}

struct RecordingState {
    current_session: Option<SessionAggregateV1>,
    terminal: Vec<TerminalReceiptV1>,
    queued: Vec<QueuedClaim>,
    requests: Vec<AdmitRequestV1>,
    sequence: u64,
    claim_attempts: u64,
}

struct QueuedClaim {
    execution: ClaimedExecutionV1,
    job: JobReceiptV1,
}

impl RecordingStore {
    fn new(identity: DurableWorktreeIdentityV1) -> Self {
        Self {
            identity,
            state: Arc::new(Mutex::new(RecordingState {
                current_session: None,
                terminal: Vec::new(),
                queued: Vec::new(),
                requests: Vec::new(),
                sequence: 0,
                claim_attempts: 0,
            })),
        }
    }

    fn current_session(&self) -> Option<SessionAggregateV1> {
        self.state.lock().unwrap().current_session.clone()
    }

    fn terminal(&self) -> Vec<TerminalReceiptV1> {
        self.state.lock().unwrap().terminal.clone()
    }

    fn request_count(&self) -> usize {
        self.state.lock().unwrap().requests.len()
    }
    fn claim_attempts(&self) -> u64 {
        self.state.lock().unwrap().claim_attempts
    }
    fn first_canonical_execution(&self) -> String {
        self.state
            .lock()
            .unwrap()
            .requests
            .first()
            .unwrap()
            .canonical_execution()
            .as_str()
            .to_owned()
    }

    fn enqueue_persisted_session_start(&self, canonical_execution: &str) -> JobId {
        let mut state = self.state.lock().unwrap();
        state.sequence += 1;
        let job = JobReceiptV1::new(
            state.sequence,
            JobId::new(format!("00000000-0000-4000-8000-{:012}", state.sequence)).unwrap(),
            Sha256Digest::new(DIGEST).unwrap(),
        );
        let job_id = job.job_id().clone();
        state.queued.push(QueuedClaim {
            execution: ClaimedExecutionV1::new_with_canonical_execution(
                DomainCommand::SessionStart,
                RevisionAttemptItemPreconditionsV1::new(None, None, None, None).unwrap(),
                CanonicalExecutionJsonV1::new(canonical_execution).unwrap(),
            ),
            job,
        });
        job_id
    }
}

impl StoreContractV1 for RecordingStore {
    fn admit(
        &self,
        identity: &DurableWorktreeIdentityV1,
        request: AdmitRequestV1,
    ) -> Result<AdmitOutcomeV1, StoreErrorV1> {
        assert_eq!(identity, &self.identity);
        let mut state = self.state.lock().unwrap();
        if let Some(existing) = state.requests.iter().find(|existing| {
            existing.idempotency_key().as_str() == request.idempotency_key().as_str()
        }) {
            assert_eq!(existing.request_digest(), request.request_digest());
            let job = state
                .terminal
                .iter()
                .find(|terminal| terminal.job().job_id() == existing.job_id())
                .map(PersistedTerminalReceiptV1::from_terminal_receipt)
                .map(JobReceiptOrTerminalV1::TerminalReceipt)
                .unwrap_or_else(|| {
                    JobReceiptOrTerminalV1::JobReceipt(JobReceiptV1::new(
                        1,
                        existing.job_id().clone(),
                        existing.request_digest().clone(),
                    ))
                });
            return Ok(AdmitOutcomeV1::Existing(job));
        }
        let actual_session_id = state
            .current_session
            .as_ref()
            .map(SessionAggregateV1::session_id)
            .cloned();
        let matches_session = match request.session_identity() {
            AdmissionSessionIdentityV1::Any => true,
            AdmissionSessionIdentityV1::Absent => actual_session_id.is_none(),
            AdmissionSessionIdentityV1::Exact(expected) => {
                actual_session_id.as_ref() == Some(expected)
            }
        };
        if !matches_session {
            return Err(StoreErrorV1::SessionIdentityConflictV1 {
                expected: match request.session_identity() {
                    AdmissionSessionIdentityV1::Exact(expected) => Some(expected.clone()),
                    AdmissionSessionIdentityV1::Any | AdmissionSessionIdentityV1::Absent => None,
                },
                actual: actual_session_id,
            });
        }
        state.sequence += 1;
        let receipt = JobReceiptV1::new(
            state.sequence,
            request.job_id().clone(),
            request.request_digest().clone(),
        );
        state.queued.push(QueuedClaim {
            execution: ClaimedExecutionV1::new_with_canonical_execution(
                request.command().clone(),
                request.preconditions().clone(),
                request.canonical_execution().clone(),
            ),
            job: receipt.clone(),
        });
        state.requests.push(request);
        Ok(AdmitOutcomeV1::New(receipt))
    }

    fn claim_next(
        &self,
        identity: &DurableWorktreeIdentityV1,
        worker: WorkerIdV1,
        _now: UnixMillis,
    ) -> Result<Option<ClaimedJobV1>, StoreErrorV1> {
        assert_eq!(identity, &self.identity);
        let mut state = self.state.lock().unwrap();
        state.claim_attempts += 1;
        let Some(queued) = (!state.queued.is_empty()).then(|| state.queued.remove(0)) else {
            return Ok(None);
        };
        let claim = ClaimTokenV1::new(
            self.identity.clone(),
            queued.job.job_id().clone(),
            Revision::new(1),
            worker,
        );
        Ok(Some(ClaimedJobV1::new_persisted(
            claim,
            queued.job,
            queued.execution,
            state.current_session.clone(),
        )))
    }

    fn cancel_before_claim(
        &self,
        _identity: &DurableWorktreeIdentityV1,
        job: JobId,
        _expected_job_revision: Revision,
        _now: UnixMillis,
    ) -> Result<CancelOutcomeV1, StoreErrorV1> {
        Ok(CancelOutcomeV1::Cancelled(JobReceiptV1::new(
            1,
            job,
            Sha256Digest::new(DIGEST).unwrap(),
        )))
    }

    fn commit_terminal(
        &self,
        claim: ClaimTokenV1,
        _expected_workspace_revision: Revision,
        transition: Option<StateTransitionV1>,
        result: TerminalResultV1,
        _now: UnixMillis,
    ) -> Result<TerminalReceiptV1, StoreErrorV1> {
        let mut state = self.state.lock().unwrap();
        if let Some(transition) = transition {
            match transition.persisted_session_mutation() {
                PersistedSessionMutationV1::Unchanged => {}
                PersistedSessionMutationV1::Replace(session)
                | PersistedSessionMutationV1::ReplaceFresh(session) => {
                    state.current_session = Some(session.clone())
                }
                PersistedSessionMutationV1::Clear => state.current_session = None,
            }
        }
        let receipt = TerminalReceiptV1::new(
            JobReceiptV1::new(
                1,
                claim.job_id().clone(),
                Sha256Digest::new(DIGEST).unwrap(),
            ),
            result,
        );
        state.terminal.push(receipt.clone());
        Ok(receipt)
    }

    fn read_workspace_view(
        &self,
        identity: &DurableWorktreeIdentityV1,
    ) -> Result<WorkspaceViewV1, StoreErrorV1> {
        assert_eq!(identity, &self.identity);
        let state = self.state.lock().unwrap();
        let revision = state
            .current_session
            .as_ref()
            .map_or(Revision::ZERO, SessionAggregateV1::revision);
        Ok(WorkspaceViewV1::new_coherent(
            self.identity.clone(),
            WorkspaceState::new(self.identity.workspace_uuid().clone(), revision, None).unwrap(),
            state.current_session.clone(),
            state.queued.len() as u32,
            None,
            state.sequence,
            UnixMillis::new(100),
        ))
    }
}

impl StoreIdempotencyReadContractV1 for RecordingStore {
    fn read_idempotent_outcome(
        &self,
        identity: &DurableWorktreeIdentityV1,
        idempotency_key: &IdempotencyKeyV1,
        request_digest: &Sha256Digest,
    ) -> Result<Option<AdmitOutcomeV1>, StoreErrorV1> {
        assert_eq!(identity, &self.identity);
        let state = self.state.lock().unwrap();
        let Some(existing) = state
            .requests
            .iter()
            .find(|existing| existing.idempotency_key().as_str() == idempotency_key.as_str())
        else {
            return Ok(None);
        };
        if existing.request_digest() != request_digest {
            return Err(StoreErrorV1::IdempotencyDigestConflictV1 {
                expected: existing.request_digest().clone(),
                actual: request_digest.clone(),
            });
        }
        let outcome = state
            .terminal
            .iter()
            .find(|terminal| terminal.job().job_id() == existing.job_id())
            .map(PersistedTerminalReceiptV1::from_terminal_receipt)
            .map(JobReceiptOrTerminalV1::TerminalReceipt)
            .unwrap_or_else(|| {
                JobReceiptOrTerminalV1::JobReceipt(JobReceiptV1::new(
                    1,
                    existing.job_id().clone(),
                    existing.request_digest().clone(),
                ))
            });
        Ok(Some(AdmitOutcomeV1::Existing(outcome)))
    }
}

struct Harness<Artifacts = FixtureArtifacts>
where
    Artifacts: ArtifactVerifierV1,
{
    engine: DaemonExecutionEngineV1<
        RecordingStore,
        FixtureIds,
        FixtureClock,
        FixtureProcedures,
        Artifacts,
        FixtureWorkspaces,
    >,
    store: RecordingStore,
    binding: WorkspaceBindingV1,
    next_key: u64,
}

impl Harness<FixtureArtifacts> {
    fn new() -> Self {
        let identity = identity();
        let binding = binding(identity.clone());
        let store = RecordingStore::new(identity);
        let engine = DaemonExecutionEngineV1::new(
            store.clone(),
            FixtureIds::new(),
            FixtureClock::new(),
            FixtureProcedures,
            FixtureArtifacts,
            FixtureWorkspaces::stable(binding.clone()),
        );
        Self {
            engine,
            store,
            binding,
            next_key: 0,
        }
    }
}

impl<Artifacts> Harness<Artifacts>
where
    Artifacts: ArtifactVerifierV1,
{
    fn submit(
        &mut self,
        command: &str,
        payload: Value,
        preconditions: PreconditionsV1,
    ) -> TerminalReceiptV1 {
        self.next_key += 1;
        let request = slice_request(command, payload, preconditions, self.next_key);
        self.engine
            .admit(
                &request,
                IdempotencyKeyV1::new(format!("key-{}", self.next_key)).unwrap(),
            )
            .unwrap();
        self.engine
            .execute_next(&self.binding, WorkerIdV1::new("execution-test").unwrap())
            .unwrap()
            .unwrap()
    }

    fn start(&mut self) -> TerminalReceiptV1 {
        self.submit(
            "session.start",
            json!({"selector": selector_json(), "preset": "sw-dev", "task_title": "Task"}),
            PreconditionsV1::default(),
        )
    }

    fn item_preconditions(&self, item_id: &str) -> PreconditionsV1 {
        let session = self.store.current_session().unwrap();
        let attempt_id = session.active_attempt_id().unwrap().clone();
        let slot = session
            .attempts()
            .iter()
            .find(|attempt| attempt.attempt_id() == &attempt_id)
            .unwrap()
            .item_slots()
            .iter()
            .find(|slot| slot.item_id().as_str() == item_id)
            .unwrap();
        PreconditionsV1::new(
            Some(session.session_id().clone()),
            None,
            Some(attempt_id),
            Some(slot.revision()),
            None,
            None,
        )
        .unwrap()
    }

    fn session_preconditions(&self) -> PreconditionsV1 {
        let session = self.store.current_session().unwrap();
        PreconditionsV1::new(
            Some(session.session_id().clone()),
            Some(session.revision()),
            session.active_attempt_id().cloned(),
            None,
            None,
            None,
        )
        .unwrap()
    }
    #[allow(dead_code)]
    fn session_identity_preconditions(&self) -> PreconditionsV1 {
        let session = self.store.current_session().unwrap();
        PreconditionsV1::new(
            Some(session.session_id().clone()),
            Some(session.revision()),
            None,
            None,
            None,
            None,
        )
        .unwrap()
    }

    #[allow(dead_code)]
    fn session_revision_preconditions(&self) -> PreconditionsV1 {
        let session = self.store.current_session().unwrap();
        PreconditionsV1::new(
            Some(session.session_id().clone()),
            Some(session.revision()),
            None,
            None,
            None,
            None,
        )
        .unwrap()
    }

    fn check(&mut self, item_id: &str) -> TerminalReceiptV1 {
        let preconditions = self.item_preconditions(item_id);
        self.submit(
            "item.check",
            json!({"selector": selector_json(), "item_id": item_id}),
            preconditions,
        )
    }

    fn attach(&mut self) -> TerminalReceiptV1 {
        let preconditions = self.item_preconditions("proof");
        self.submit(
            "item.attach",
            json!({"selector": selector_json(), "item_id": "proof", "path": "proof.txt", "media_type": "text/plain"}),
            preconditions,
        )
    }

    fn complete(&mut self) -> TerminalReceiptV1 {
        let preconditions = self.session_preconditions();
        self.submit(
            "session.complete",
            json!({"selector": selector_json()}),
            preconditions,
        )
    }
}

fn identity() -> DurableWorktreeIdentityV1 {
    DurableWorktreeIdentityV1::new(
        Sha256Digest::new(DIGEST).unwrap(),
        WorkspaceId::new(WORKSPACE_ID).unwrap(),
        Sha256Digest::new(DIGEST).unwrap(),
    )
}

fn binding(identity: DurableWorktreeIdentityV1) -> WorkspaceBindingV1 {
    binding_at(identity, "podway.unix-path/v1:2f776f726b74726565")
}

fn binding_at(identity: DurableWorktreeIdentityV1, root: &str) -> WorkspaceBindingV1 {
    WorkspaceBindingV1::new(
        identity,
        podway_store::ValidatedWorkspaceRootV1::from_encoded(root).unwrap(),
    )
}

fn selector_json() -> Value {
    selector_json_with_expected(Some(WORKSPACE_ID))
}

#[allow(dead_code)]
fn selector_json_without_expected_uuid() -> Value {
    selector_json_with_expected(None)
}

fn selector_json_with_expected(expected_uuid: Option<&str>) -> Value {
    json!({
        "version": 1,
        "path_bytes_base64url": "L3dvcmt0cmVl",
        "display": "/worktree",
        "expected_uuid": expected_uuid,
    })
}

fn slice_request(
    command: &str,
    payload: Value,
    preconditions: PreconditionsV1,
    key: u64,
) -> SliceRequestV1 {
    let payload: Map<String, Value> = payload.as_object().unwrap().clone();
    let expected_workspace_id = payload
        .get("selector")
        .and_then(Value::as_object)
        .and_then(|selector| selector.get("expected_uuid"))
        .and_then(Value::as_str)
        .map(|value| WorkspaceId::new(value.to_owned()).expect("fixture UUID is valid"));
    let envelope = RequestEnvelopeV1::new(RequestEnvelopeInputV1 {
        request_id: RequestIdV1::new(format!("00000000-0000-4000-8000-{key:012}")).unwrap(),
        client: ClientInfoV1::new("execution-test", "1", 1).unwrap(),
        operation: if matches!(command, "workspace.init" | "workspace.reset_all") {
            OperationV1::Bootstrap
        } else {
            OperationV1::Mutate
        },
        command: CommandNameV1::new(command).unwrap(),
        workspace: Some(WorkspaceContextV1::new("/worktree", expected_workspace_id).unwrap()),
        idempotency_key: Some(ProtocolIdempotencyKeyV1::new(format!("protocol-{key}")).unwrap()),
        preconditions,
        options: RequestOptionsV1::new(false, 0).unwrap(),
        payload,
    })
    .unwrap();
    SliceRequestV1::from_envelope(&envelope).unwrap()
}

fn assert_success(receipt: &TerminalReceiptV1) {
    assert!(
        matches!(receipt.result(), TerminalResultV1::Success(_)),
        "expected success, got {:?}",
        receipt.result()
    );
}

fn assert_failure(receipt: &TerminalReceiptV1) {
    assert!(matches!(receipt.result(), TerminalResultV1::Failure(_)));
}

#[test]
fn session_identity_mismatch_is_rejected_before_durable_admission() {
    let mut harness = Harness::new();
    assert_success(&harness.start());
    let current = harness.store.current_session().unwrap();
    let requests_before = harness.store.request_count();
    let foreign = SessionId::new("00000000-0000-4000-8000-000000000099").unwrap();
    let request = slice_request(
        "session.cancel",
        json!({"selector": selector_json(), "reason": "wrong identity"}),
        PreconditionsV1::new(
            Some(foreign.clone()),
            Some(current.revision()),
            Some(current.active_attempt_id().unwrap().clone()),
            None,
            None,
            None,
        )
        .unwrap(),
        700,
    );

    let error = harness
        .engine
        .admit(&request, IdempotencyKeyV1::new("foreign-session").unwrap())
        .unwrap_err();

    assert!(matches!(
        error,
        ExecutionErrorV1::SessionIdentityMismatch {
            expected,
            actual: Some(actual),
        } if expected == foreign && actual == *current.session_id()
    ));
    assert_eq!(harness.store.request_count(), requests_before);
    assert_eq!(harness.store.current_session(), Some(current));
}

#[test]
fn workspace_identity_mismatch_survives_admission_revalidation() {
    let durable_identity = identity();
    let binding = binding(durable_identity.clone());
    let store = RecordingStore::new(durable_identity);
    let workspaces = FixtureWorkspaces::stable(binding);
    let expected = WorkspaceId::new(WORKSPACE_ID).unwrap();
    let actual = WorkspaceId::new("00000000-0000-4000-8000-000000000099").unwrap();
    workspaces.mismatch_selector_identity(expected.clone(), actual.clone());
    let engine = DaemonExecutionEngineV1::new(
        store.clone(),
        FixtureIds::new(),
        FixtureClock::new(),
        FixtureProcedures,
        FixtureArtifacts,
        workspaces,
    );
    let request = slice_request(
        "session.start",
        json!({"selector": selector_json(), "preset": "sw-dev", "task_title": "Task"}),
        PreconditionsV1::default(),
        701,
    );

    let error = engine
        .admit(
            &request,
            IdempotencyKeyV1::new("foreign-workspace").unwrap(),
        )
        .unwrap_err();

    assert!(matches!(
        error,
        ExecutionErrorV1::WorkspaceIdentityMismatch {
            expected: error_expected,
            actual: error_actual,
        } if error_expected == expected && error_actual == actual
    ));
    assert_eq!(store.request_count(), 0);
}

#[test]
fn session_start_with_an_existing_session_is_rejected_before_admission() {
    let mut harness = Harness::new();
    assert_success(&harness.start());
    let current = harness.store.current_session();
    let requests_before = harness.store.request_count();
    let request = slice_request(
        "session.start",
        json!({"selector": selector_json(), "preset": "sw-dev", "task_title": "Second"}),
        PreconditionsV1::default(),
        701,
    );

    let error = harness
        .engine
        .admit(&request, IdempotencyKeyV1::new("second-start").unwrap())
        .unwrap_err();

    assert!(matches!(
        error,
        ExecutionErrorV1::Store(StoreErrorV1::SessionIdentityConflictV1 {
            expected: None,
            actual: Some(_),
        })
    ));
    assert_eq!(harness.store.request_count(), requests_before);
    assert_eq!(harness.store.current_session(), current);
}

#[test]
fn clean_start_is_claimed_decoded_and_committed() {
    let mut harness = Harness::new();
    assert_success(&harness.start());
    assert_eq!(
        harness.store.current_session().unwrap().revision(),
        Revision::new(1)
    );
}
#[test]
fn literal_legacy_v1_and_v2_documents_fail_closed_without_regeneration() {
    for document in [
        LEGACY_V1_SESSION_START_DOCUMENT,
        LEGACY_V2_SESSION_START_DOCUMENT,
    ] {
        let identity = identity();
        let binding = binding(identity.clone());
        let store = RecordingStore::new(identity);
        let ids = SpyIds::new();
        let procedures = SpyProcedures::new();
        let engine = DaemonExecutionEngineV1::new(
            store.clone(),
            ids.clone(),
            FixtureClock::new(),
            procedures.clone(),
            FixtureArtifacts,
            FixtureWorkspaces::stable(binding.clone()),
        );

        store.enqueue_persisted_session_start(document);
        store.enqueue_persisted_session_start(document);
        for worker in ["legacy-recovery-one", "legacy-recovery-two"] {
            assert!(matches!(
                engine.execute_next(&binding, WorkerIdV1::new(worker).unwrap()),
                Err(ExecutionErrorV1::InvalidPersistedExecution {
                    reason: "legacy execution document lacks immutable admission resolution"
                })
            ));
        }
        assert_eq!(store.claim_attempts(), 2);
        assert!(store.terminal().is_empty());
        assert!(store.current_session().is_none());
        assert_eq!(procedures.call_count(), 0);
        assert!(ids.calls().is_empty());
    }
}

#[test]
fn v4_recovery_uses_the_persisted_snapshot_and_ids_without_regeneration() {
    let identity = identity();
    let binding = binding(identity.clone());
    let store = RecordingStore::new(identity);
    let admitted = DaemonExecutionEngineV1::new(
        store.clone(),
        FixtureIds::new(),
        FixtureClock::new(),
        FixtureProcedures,
        FixtureArtifacts,
        FixtureWorkspaces::stable(binding.clone()),
    );
    let request = slice_request(
        "session.start",
        json!({"selector": selector_json(), "preset": "sw-dev", "task_title": "Persisted"}),
        PreconditionsV1::default(),
        950,
    );
    admitted
        .admit(&request, IdempotencyKeyV1::new("persisted-v4").unwrap())
        .unwrap();

    let execution_ids = SpyIds::new();
    let execution_procedures = SpyProcedures::new();
    let restarted = DaemonExecutionEngineV1::new(
        store.clone(),
        execution_ids.clone(),
        FixtureClock::new(),
        execution_procedures.clone(),
        FixtureArtifacts,
        FixtureWorkspaces::stable(binding.clone()),
    );
    assert_success(
        &restarted
            .execute_next(&binding, WorkerIdV1::new("v4-recovery").unwrap())
            .unwrap()
            .unwrap(),
    );
    assert_eq!(execution_procedures.call_count(), 0);
    assert!(execution_ids.calls().is_empty());

    let session = store.current_session().unwrap();
    assert_eq!(
        session.snapshot().snapshot_id().as_str(),
        "00000000-0000-4000-8000-000000000011"
    );
    assert_eq!(
        session.session_id().as_str(),
        "00000000-0000-4000-8000-000000000012"
    );
    assert_eq!(
        session.active_attempt_id().unwrap().as_str(),
        "00000000-0000-4000-8000-000000000013"
    );
}
#[test]
fn legacy_execution_version_with_a_v5_resolution_is_rejected() {
    let identity = identity();
    let binding = binding(identity.clone());
    let source_store = RecordingStore::new(identity.clone());
    let admitted = DaemonExecutionEngineV1::new(
        source_store.clone(),
        FixtureIds::new(),
        FixtureClock::new(),
        FixtureProcedures,
        FixtureArtifacts,
        FixtureWorkspaces::stable(binding.clone()),
    );
    let request = slice_request(
        "session.start",
        json!({"selector": selector_json(), "preset": "sw-dev", "task_title": "Unsupported"}),
        PreconditionsV1::default(),
        954,
    );
    admitted
        .admit(&request, IdempotencyKeyV1::new("unsupported-v2").unwrap())
        .unwrap();
    let unsupported = source_store.first_canonical_execution().replacen(
        "\"execution_version\":5",
        "\"execution_version\":3",
        1,
    );

    let recovery_store = RecordingStore::new(identity);
    let ids = SpyIds::new();
    let procedures = SpyProcedures::new();
    let recovery = DaemonExecutionEngineV1::new(
        recovery_store.clone(),
        ids.clone(),
        FixtureClock::new(),
        procedures.clone(),
        FixtureArtifacts,
        FixtureWorkspaces::stable(binding.clone()),
    );
    recovery_store.enqueue_persisted_session_start(&unsupported);
    assert!(matches!(
        recovery.execute_next(&binding, WorkerIdV1::new("unsupported-v3").unwrap()),
        Err(ExecutionErrorV1::InvalidPersistedExecution {
            reason: "legacy execution document lacks session identity fences"
        })
    ));
    assert_eq!(procedures.call_count(), 0);
    assert!(ids.calls().is_empty());
}

#[test]
fn moved_queued_work_uses_the_rediscovered_manager_binding_not_its_stale_selector() {
    let identity = identity();
    let admitted_binding = binding(identity.clone());
    let moved_binding = binding_at(
        identity.clone(),
        "podway.unix-path/v1:2f6d6f7665642d776f726b74726565",
    );
    let store = RecordingStore::new(identity);
    let workspaces = FixtureWorkspaces::moved(admitted_binding.clone(), moved_binding.clone());
    let engine = DaemonExecutionEngineV1::new(
        store.clone(),
        FixtureIds::new(),
        FixtureClock::new(),
        FixtureProcedures,
        FixtureArtifacts,
        workspaces.clone(),
    );
    let request = slice_request(
        "session.start",
        json!({"selector": selector_json(), "preset": "sw-dev", "task_title": "Moved"}),
        PreconditionsV1::default(),
        951,
    );
    engine
        .admit(&request, IdempotencyKeyV1::new("moved-worktree").unwrap())
        .unwrap();
    workspaces.reject_stale_selectors();

    assert_success(
        &engine
            .execute_next(&moved_binding, WorkerIdV1::new("moved-worktree").unwrap())
            .unwrap()
            .unwrap(),
    );
    assert_eq!(
        workspaces.selector_revalidations.load(Ordering::SeqCst),
        1,
        "only admission may inspect the stale selector"
    );
    assert_eq!(workspaces.binding_revalidations.load(Ordering::SeqCst), 1);
}

#[test]
fn deleted_or_replaced_manager_binding_fails_before_any_store_claim() {
    let durable_identity = identity();
    let workspace_binding = binding(durable_identity.clone());
    let store = RecordingStore::new(durable_identity);
    let deleted = FixtureWorkspaces::stable(workspace_binding.clone());
    let engine = DaemonExecutionEngineV1::new(
        store.clone(),
        FixtureIds::new(),
        FixtureClock::new(),
        FixtureProcedures,
        FixtureArtifacts,
        deleted.clone(),
    );
    let request = slice_request(
        "session.start",
        json!({"selector": selector_json(), "preset": "sw-dev", "task_title": "Deleted"}),
        PreconditionsV1::default(),
        952,
    );
    engine
        .admit(&request, IdempotencyKeyV1::new("deleted-worktree").unwrap())
        .unwrap();
    deleted.reject_manager_binding();
    assert!(matches!(
        engine.execute_next(
            &workspace_binding,
            WorkerIdV1::new("deleted-worktree").unwrap()
        ),
        Err(ExecutionErrorV1::BoundaryDomain(_))
    ));
    assert_eq!(store.claim_attempts(), 0);

    let replacement_identity = identity();
    let original_binding = binding(replacement_identity.clone());
    let replacement_binding = binding_at(
        replacement_identity.clone(),
        "podway.unix-path/v1:2f7265706c6163656d656e74",
    );
    let replacement_store = RecordingStore::new(replacement_identity);
    let replaced = FixtureWorkspaces::moved(original_binding.clone(), replacement_binding);
    let replacement_engine = DaemonExecutionEngineV1::new(
        replacement_store.clone(),
        FixtureIds::new(),
        FixtureClock::new(),
        FixtureProcedures,
        FixtureArtifacts,
        replaced,
    );
    let replacement_request = slice_request(
        "session.start",
        json!({"selector": selector_json(), "preset": "sw-dev", "task_title": "Replacement"}),
        PreconditionsV1::default(),
        953,
    );
    replacement_engine
        .admit(
            &replacement_request,
            IdempotencyKeyV1::new("replaced-worktree").unwrap(),
        )
        .unwrap();
    assert!(matches!(
        replacement_engine.execute_next(
            &original_binding,
            WorkerIdV1::new("replaced-worktree").unwrap(),
        ),
        Err(ExecutionErrorV1::BoundaryDomain(_))
    ));
    assert_eq!(replacement_store.claim_attempts(), 0);
}

#[test]
fn distinctive_item_mutations_use_the_typed_core_command() {
    let mut harness = Harness::new();
    harness.start();
    let preconditions = harness.item_preconditions("entries");
    assert_success(&harness.submit(
        "item.add",
        json!({"selector": selector_json(), "item_id": "entries", "value": "distinct"}),
        preconditions,
    ));
    let preconditions = harness.item_preconditions("notes");
    assert_success(&harness.submit(
        "item.set",
        json!({"selector": selector_json(), "item_id": "notes", "value": "typed"}),
        preconditions,
    ));
    assert_success(&harness.attach());
}

#[test]
fn clean_retry_starts_an_empty_attempt_without_copying_item_values() {
    let mut harness = Harness::new();
    harness.start();
    harness.check("confirm");
    let previous_attempt = harness
        .store
        .current_session()
        .unwrap()
        .active_attempt_id()
        .unwrap()
        .clone();
    let preconditions = harness.session_preconditions();
    assert_success(&harness.submit(
        "session.retry",
        json!({"selector": selector_json(), "reason": "redo"}),
        preconditions,
    ));
    let session = harness.store.current_session().unwrap();
    assert_ne!(session.active_attempt_id(), Some(&previous_attempt));
    assert!(
        session
            .attempts()
            .iter()
            .find(|attempt| Some(attempt.attempt_id()) == session.active_attempt_id())
            .unwrap()
            .item_slots()
            .iter()
            .all(|slot| slot.value().is_none())
    );
}

#[test]
fn return_marks_rework_and_starts_a_fresh_destination_attempt() {
    let mut harness = Harness::new();
    harness.start();
    harness.check("confirm");
    harness.attach();
    harness.complete();
    harness.check("finish");
    let preconditions = harness.session_preconditions();
    assert_success(&harness.submit(
        "session.return",
        json!({"selector": selector_json(), "destination_stage_id": "first", "reason": "rework"}),
        preconditions,
    ));
    let session = harness.store.current_session().unwrap();
    assert_eq!(session.active_stage_id().unwrap().as_str(), "first");
    assert!(
        session
            .attempts()
            .iter()
            .find(|attempt| Some(attempt.attempt_id()) == session.active_attempt_id())
            .unwrap()
            .item_slots()
            .iter()
            .all(|slot| slot.value().is_none())
    );
}

#[test]
fn completion_domain_failures_are_terminal_and_do_not_mutate_state() {
    let mut harness = Harness::new();
    harness.start();
    let before = harness.store.current_session().unwrap();
    assert_failure(&harness.complete());
    assert_eq!(harness.store.current_session().unwrap(), before);
    let preconditions = harness.session_preconditions();
    assert_success(&harness.submit(
        "session.block",
        json!({"selector": selector_json(), "reason": "blocked"}),
        preconditions,
    ));
    assert_failure(&harness.complete());
}

#[test]
fn artifact_completion_failure_is_terminal_when_revalidation_rejects() {
    #[derive(Clone, Copy)]
    struct RejectingArtifacts;
    impl ArtifactVerifierV1 for RejectingArtifacts {
        fn hash_local_artifact(
            &self,
            workspace: &WorkspaceBindingV1,
            path: &str,
            media_type: Option<&str>,
        ) -> Result<ArtifactValueV1, ExecutionBoundaryErrorV1> {
            FixtureArtifacts.hash_local_artifact(workspace, path, media_type)
        }
        fn revalidate_local_artifact(
            &self,
            _workspace: &WorkspaceBindingV1,
            _item_id: &podway_core::ItemId,
            _artifact: &ArtifactValueV1,
        ) -> Result<LocalArtifactVerificationV1, ExecutionBoundaryErrorV1> {
            Err(ExecutionBoundaryErrorV1::domain(
                DomainError::InvalidState {
                    reason: "artifact changed after attachment",
                },
            ))
        }
    }
    let identity = identity();
    let binding = binding(identity.clone());
    let store = RecordingStore::new(identity);
    let engine = DaemonExecutionEngineV1::new(
        store.clone(),
        FixtureIds::new(),
        FixtureClock::new(),
        FixtureProcedures,
        RejectingArtifacts,
        FixtureWorkspaces::stable(binding.clone()),
    );
    let mut harness = Harness {
        engine,
        store,
        binding,
        next_key: 0,
    };
    harness.start();
    harness.check("confirm");
    harness.attach();
    assert_failure(&harness.complete());
}

#[test]
fn stale_preconditions_commit_a_terminal_failure_without_state_replacement() {
    let mut harness = Harness::new();
    harness.start();
    let stale = harness.item_preconditions("confirm");
    harness.check("confirm");
    let before = harness.store.current_session().unwrap();
    assert_failure(&harness.submit(
        "item.check",
        json!({"selector": selector_json(), "item_id": "confirm"}),
        stale,
    ));
    assert_eq!(harness.store.current_session().unwrap(), before);
}

#[test]
fn successful_completion_commits_the_transition_and_terminal_receipt_together() {
    let mut harness = Harness::new();
    harness.start();
    harness.check("confirm");
    harness.attach();
    let before_receipts = harness.store.terminal().len();
    assert_success(&harness.complete());
    assert_eq!(harness.store.terminal().len(), before_receipts + 1);
    assert_eq!(
        harness
            .store
            .current_session()
            .unwrap()
            .active_stage_id()
            .unwrap()
            .as_str(),
        "second"
    );
}
#[test]
fn terminal_idempotency_replay_returns_the_exact_receipt_without_fresh_dependencies() {
    let identity = identity();
    let binding = binding(identity.clone());
    let store = RecordingStore::new(identity);
    let request = slice_request(
        "session.start",
        json!({"selector": selector_json(), "preset": "sw-dev", "task_title": "Task"}),
        PreconditionsV1::default(),
        900,
    );
    let key = IdempotencyKeyV1::new("immutable-terminal").unwrap();
    let admitted = DaemonExecutionEngineV1::new(
        store.clone(),
        FixtureIds::new(),
        FixtureClock::new(),
        FixtureProcedures,
        FixtureArtifacts,
        FixtureWorkspaces::stable(binding.clone()),
    );
    admitted
        .admit_for_workspace(&binding, &request, key.clone())
        .unwrap();
    let terminal = admitted
        .execute_next(&binding, WorkerIdV1::new("execution-test").unwrap())
        .unwrap()
        .unwrap();

    let replay_ids = SpyIds::new();
    let replay_clock = SpyClock::new();
    let replay_procedures = SpyProcedures::new();
    let replay_workspaces = FixtureWorkspaces::stable(binding.clone());
    replay_workspaces.reject_stale_selectors();
    let replay_engine = DaemonExecutionEngineV1::new(
        store.clone(),
        replay_ids.clone(),
        replay_clock.clone(),
        replay_procedures.clone(),
        FixtureArtifacts,
        replay_workspaces.clone(),
    );
    let replay = replay_engine
        .admit_for_workspace(&binding, &request, key)
        .unwrap();

    assert_eq!(
        replay,
        AdmitOutcomeV1::Existing(JobReceiptOrTerminalV1::TerminalReceipt(
            PersistedTerminalReceiptV1::from_terminal_receipt(&terminal)
        ))
    );
    assert_eq!(
        replay_workspaces
            .selector_revalidations
            .load(Ordering::SeqCst),
        0
    );
    assert_eq!(replay_procedures.call_count(), 0);
    assert_eq!(replay_clock.call_count(), 0);
    assert!(replay_ids.calls().is_empty());
    assert_eq!(store.request_count(), 1);
    assert_eq!(store.terminal(), vec![terminal]);
}

#[test]
fn a_claimed_execution_survives_engine_restart_and_decodes_its_immutable_document() {
    let identity = identity();
    let binding = binding(identity.clone());
    let store = RecordingStore::new(identity);
    let first = DaemonExecutionEngineV1::new(
        store.clone(),
        FixtureIds::new(),
        FixtureClock::new(),
        FixtureProcedures,
        FixtureArtifacts,
        FixtureWorkspaces::stable(binding.clone()),
    );
    let request = slice_request(
        "session.start",
        json!({"selector": selector_json(), "preset": "sw-dev", "task_title": "Restart"}),
        PreconditionsV1::default(),
        901,
    );
    first
        .admit(&request, IdempotencyKeyV1::new("restart-safe").unwrap())
        .unwrap();
    let restarted = DaemonExecutionEngineV1::new(
        store.clone(),
        FixtureIds::new(),
        FixtureClock::new(),
        FixtureProcedures,
        FixtureArtifacts,
        FixtureWorkspaces::stable(binding.clone()),
    );
    assert_success(
        &restarted
            .execute_next(&binding, WorkerIdV1::new("restarted").unwrap())
            .unwrap()
            .unwrap(),
    );
}

#[test]
fn workspace_initialization_commits_a_success_with_no_session_mutation() {
    let mut harness = Harness::new();
    assert_success(&harness.submit(
        "workspace.init",
        json!({"selector": selector_json()}),
        PreconditionsV1::default(),
    ));
    assert!(harness.store.current_session().is_none());
}

#[test]
fn embedded_provider_uses_the_public_config_and_preset_admission_path() {
    let snapshot = EmbeddedPresetProcedureProviderV1
        .load_preset_snapshot(
            "sw-dev",
            ProcedureSnapshotId::new("00000000-0000-4000-8000-000000000099").unwrap(),
            UnixMillis::new(100),
        )
        .unwrap();
    assert_eq!(snapshot.procedure_id(), "sw-dev");
}
