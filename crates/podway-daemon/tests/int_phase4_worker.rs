//! Phase 4 worker contracts over runtime-manager scheduler handles.
//!
//! The fixture has durable job rows, but no worker-owned scheduler registry or queue. Each test
//! explicitly creates a scheduler in one identity-keyed registry, matching production composition.

use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc, Barrier, Condvar, Mutex,
        atomic::{AtomicU64, AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};

use podway_core::{DomainCommand, DomainError, JobId, Sha256Digest, UnixMillis, WorkspaceId};
use podway_daemon::{
    execution::ExecutionErrorV1,
    read_service::{MonotonicClockV1, MonotonicDeadlineV1},
    scheduler::{WorkspaceSchedulerRegistryV1, WorkspaceSchedulerV1},
    worker::{
        DaemonWorkerV1, WorkerClockV1, WorkerCompletionModeV1, WorkerErrorV1, WorkerExecutionV1,
        WorkerRetirementErrorV1, WorkerWaitResultV1, WorkerWorkspaceContextV1,
        WorkspaceSchedulerKeyV1,
    },
};
use podway_protocol::{
    ClientInfoV1, CommandNameV1, IdempotencyKeyV1 as ProtocolIdempotencyKeyV1, OperationV1,
    PreconditionsV1, RequestEnvelopeInputV1, RequestEnvelopeV1, RequestIdV1, RequestOptionsV1,
    SliceRequestV1, WorkspaceContextV1,
};
use podway_store::{
    AdmitOutcomeV1, ClaimedExecutionV1, DurableWorktreeIdentityV1, IdempotencyKeyV1,
    JobReceiptOrTerminalV1, JobReceiptV1, JobStateV1, JobViewV1, PersistedTerminalReceiptV1,
    RevisionAttemptItemPreconditionsV1, StoreErrorV1, TerminalReceiptV1, TerminalResultV1,
    WorkerIdV1, WorkspaceBindingV1,
};
use serde_json::{Map, Value, json};

#[derive(Clone)]
struct FixtureJob {
    key: WorkspaceSchedulerKeyV1,
    receipt: JobReceiptV1,
    state: JobStateV1,
    terminal: Option<PersistedTerminalReceiptV1>,
}

enum Gate {
    None,
    First(Option<(mpsc::Sender<()>, mpsc::Receiver<()>)>),
    Every(Arc<Barrier>),
}

enum GateAction {
    None,
    First(mpsc::Sender<()>, mpsc::Receiver<()>),
    Every(Arc<Barrier>),
}
#[derive(Clone, Copy)]
enum RecoveryHookPanic {
    StopClaims,
    MarkRecoveryRequired,
}
enum FixtureMaintenanceStoreState {
    Open {
        close_result: Result<(), StoreErrorV1>,
    },
    Closed(Result<(), StoreErrorV1>),
}

struct FixtureState {
    next_sequence: u64,
    queues: HashMap<WorkspaceSchedulerKeyV1, VecDeque<JobId>>,
    jobs: HashMap<JobId, FixtureJob>,
    executed: Vec<JobId>,
    gate: Gate,
    transient_once: bool,
    panic_once: bool,
    admission_key: Option<WorkspaceSchedulerKeyV1>,
    admitted: Option<JobId>,
    admission_committed: bool,
}

struct FixtureBoundary {
    state: Mutex<FixtureState>,
    admission_gate: Mutex<Option<(mpsc::Sender<()>, mpsc::Receiver<()>)>>,
    active_executions: AtomicUsize,
    recovery_marker: Mutex<Option<mpsc::Sender<()>>>,
    maximum_active_executions: AtomicUsize,
}

impl FixtureBoundary {
    fn new() -> Self {
        Self {
            state: Mutex::new(FixtureState {
                next_sequence: 1,
                queues: HashMap::new(),
                jobs: HashMap::new(),
                executed: Vec::new(),
                gate: Gate::None,
                transient_once: false,
                admission_key: None,
                panic_once: false,
                admitted: None,
                admission_committed: false,
            }),
            admission_gate: Mutex::new(None),
            recovery_marker: Mutex::new(None),
            active_executions: AtomicUsize::new(0),
            maximum_active_executions: AtomicUsize::new(0),
        }
    }

    fn enqueue(&self, binding: &WorkspaceBindingV1) -> JobReceiptV1 {
        let key = WorkspaceSchedulerKeyV1::from_durable_identity(binding.identity());
        enqueue_for_key(&mut mutex_lock(&self.state), key)
    }

    fn configure_admission(&self, binding: &WorkspaceBindingV1) {
        mutex_lock(&self.state).admission_key = Some(
            WorkspaceSchedulerKeyV1::from_durable_identity(binding.identity()),
        );
    }
    fn gate_admission(&self) -> (mpsc::Receiver<()>, mpsc::Sender<()>) {
        let (entered_sender, entered_receiver) = mpsc::channel();
        let (release_sender, release_receiver) = mpsc::channel();
        *mutex_lock(&self.admission_gate) = Some((entered_sender, release_receiver));
        (entered_receiver, release_sender)
    }

    fn first_execution_gate(&self) -> (mpsc::Receiver<()>, mpsc::Sender<()>) {
        let (entered_sender, entered_receiver) = mpsc::channel();
        let (release_sender, release_receiver) = mpsc::channel();
        mutex_lock(&self.state).gate = Gate::First(Some((entered_sender, release_receiver)));
        (entered_receiver, release_sender)
    }

    fn overlap_gate(&self, participants: usize) {
        mutex_lock(&self.state).gate = Gate::Every(Arc::new(Barrier::new(participants)));
    }

    fn make_first_execution_transient(&self) {
        mutex_lock(&self.state).transient_once = true;
    }

    fn panic_on_next_execution(&self) -> mpsc::Receiver<()> {
        let (marked, observed) = mpsc::channel();
        mutex_lock(&self.state).panic_once = true;
        *mutex_lock(&self.recovery_marker) = Some(marked);
        observed
    }

    fn recover_running_to_queued(&self) {
        let mut state = mutex_lock(&self.state);
        let recovered: Vec<_> = state
            .jobs
            .iter()
            .filter_map(|(job, record)| {
                (record.state == JobStateV1::Running).then_some(job.clone())
            })
            .collect();
        for job in recovered {
            let key = {
                let record = state
                    .jobs
                    .get_mut(&job)
                    .expect("recovered job remains present");
                record.state = JobStateV1::Queued;
                record.key.clone()
            };
            state.queues.entry(key).or_default().push_back(job);
        }
    }

    fn job_state(&self, job: &JobId) -> JobStateV1 {
        mutex_lock(&self.state)
            .jobs
            .get(job)
            .expect("fixture job remains present")
            .state
    }

    fn executed(&self) -> Vec<JobId> {
        mutex_lock(&self.state).executed.clone()
    }

    fn admission_committed(&self) -> bool {
        mutex_lock(&self.state).admission_committed
    }

    fn read_job(&self, job: &JobId) -> Option<JobViewV1> {
        mutex_lock(&self.state).jobs.get(job).cloned().map(job_view)
    }
}

struct FixtureCoordination {
    claim_gate: Mutex<()>,
    state: Mutex<FixtureCoordinationState>,
    changed: Condvar,
}

struct FixtureCoordinationState {
    accepting_claims: bool,
    recovery_required: bool,
    notification_version: u64,
    stop_claims_calls: usize,
    mark_recovery_required_calls: usize,
    panic_stop_claims_once: bool,
    panic_mark_recovery_required_once: bool,
    maintenance_store: FixtureMaintenanceStoreState,
    maintenance_close_calls: usize,
}

struct FixtureContext {
    binding: WorkspaceBindingV1,
    boundary: Arc<FixtureBoundary>,
    coordination: Arc<FixtureCoordination>,
}

impl FixtureContext {
    fn new(binding: WorkspaceBindingV1, boundary: Arc<FixtureBoundary>) -> Self {
        Self {
            binding,
            boundary,
            coordination: Arc::new(FixtureCoordination {
                claim_gate: Mutex::new(()),
                state: Mutex::new(FixtureCoordinationState {
                    accepting_claims: true,
                    recovery_required: false,
                    notification_version: 0,
                    stop_claims_calls: 0,
                    mark_recovery_required_calls: 0,
                    panic_stop_claims_once: false,
                    panic_mark_recovery_required_once: false,
                    maintenance_store: FixtureMaintenanceStoreState::Open {
                        close_result: Ok(()),
                    },
                    maintenance_close_calls: 0,
                }),
                changed: Condvar::new(),
            }),
        }
    }

    fn panic_recovery_hook_once(&self, hook: RecoveryHookPanic) {
        let mut state = mutex_lock(&self.coordination.state);
        match hook {
            RecoveryHookPanic::StopClaims => state.panic_stop_claims_once = true,
            RecoveryHookPanic::MarkRecoveryRequired => {
                state.panic_mark_recovery_required_once = true;
            }
        }
    }

    fn recovery_hook_call_counts(&self) -> (usize, usize) {
        let state = mutex_lock(&self.coordination.state);
        (state.stop_claims_calls, state.mark_recovery_required_calls)
    }

    fn claims_are_stopped(&self) -> bool {
        !mutex_lock(&self.coordination.state).accepting_claims
    }
    fn fail_maintenance_store_close(&self, error: StoreErrorV1) {
        let mut state = mutex_lock(&self.coordination.state);
        match &mut state.maintenance_store {
            FixtureMaintenanceStoreState::Open { close_result } => {
                *close_result = Err(error);
            }
            FixtureMaintenanceStoreState::Closed(_) => {
                panic!("maintenance Store close failure must be configured before close");
            }
        }
    }

    fn maintenance_store_is_closed(&self) -> bool {
        matches!(
            &mutex_lock(&self.coordination.state).maintenance_store,
            FixtureMaintenanceStoreState::Closed(_)
        )
    }

    fn maintenance_close_call_count(&self) -> usize {
        mutex_lock(&self.coordination.state).maintenance_close_calls
    }
}

impl WorkerWorkspaceContextV1 for FixtureContext {
    fn binding(&self) -> &WorkspaceBindingV1 {
        &self.binding
    }

    fn read_job(&self, job: &JobId) -> Result<Option<JobViewV1>, StoreErrorV1> {
        Ok(self.boundary.read_job(job))
    }

    fn with_claim_permission<R>(
        &self,
        operation: impl FnOnce(&WorkspaceBindingV1) -> R,
    ) -> Option<R> {
        let _claim_gate = mutex_lock(&self.coordination.claim_gate);
        if !mutex_lock(&self.coordination.state).accepting_claims {
            return None;
        }
        Some(operation(&self.binding))
    }

    fn stop_claims(&self) {
        let panics_after_stopping_claims = {
            let _claim_gate = mutex_lock(&self.coordination.claim_gate);
            let mut state = mutex_lock(&self.coordination.state);
            state.stop_claims_calls += 1;
            state.accepting_claims = false;
            std::mem::replace(&mut state.panic_stop_claims_once, false)
        };
        assert!(
            !panics_after_stopping_claims,
            "fixture injected stop_claims panic"
        );
    }
    fn close_store_for_maintenance(&self) -> Result<(), StoreErrorV1> {
        let mut state = mutex_lock(&self.coordination.state);
        state.maintenance_close_calls += 1;
        let maintenance_store = std::mem::replace(
            &mut state.maintenance_store,
            FixtureMaintenanceStoreState::Closed(Ok(())),
        );
        let result = match maintenance_store {
            FixtureMaintenanceStoreState::Open { close_result }
            | FixtureMaintenanceStoreState::Closed(close_result) => close_result,
        };
        state.maintenance_store = FixtureMaintenanceStoreState::Closed(result.clone());
        result
    }

    fn mark_recovery_required(&self) {
        let (panics_after_marking_recovery, marked) = {
            let mut state = mutex_lock(&self.coordination.state);
            state.mark_recovery_required_calls += 1;
            state.recovery_required = true;
            let panics_after_marking_recovery =
                std::mem::replace(&mut state.panic_mark_recovery_required_once, false);
            drop(state);
            (
                panics_after_marking_recovery,
                mutex_lock(&self.boundary.recovery_marker).take(),
            )
        };
        if let Some(marked) = marked {
            marked
                .send(())
                .expect("test awaits the detached panic recovery mark");
        }
        assert!(
            !panics_after_marking_recovery,
            "fixture injected mark_recovery_required panic"
        );
    }

    fn recovery_required(&self) -> bool {
        mutex_lock(&self.coordination.state).recovery_required
    }

    fn notification_version(&self) -> u64 {
        mutex_lock(&self.coordination.state).notification_version
    }

    fn wait_for_notification_after(&self, observed: u64, timeout: Duration) {
        let state = mutex_lock(&self.coordination.state);
        if state.notification_version == observed {
            let _ = self
                .coordination
                .changed
                .wait_timeout(state, timeout)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
    }

    fn notify_after_authoritative_change(&self) {
        let mut state = mutex_lock(&self.coordination.state);
        state.notification_version = state.notification_version.wrapping_add(1);
        drop(state);
        self.coordination.changed.notify_all();
    }
}

impl WorkerExecutionV1<FixtureContext> for FixtureBoundary {
    fn admit(
        &self,
        workspace: &FixtureContext,
        binding: &WorkspaceBindingV1,
        _request: &SliceRequestV1,
        _idempotency_key: IdempotencyKeyV1,
        _response_context: Option<&podway_store::PersistedResponseContextV1>,
    ) -> Result<AdmitOutcomeV1, ExecutionErrorV1> {
        assert_eq!(
            workspace.binding(),
            binding,
            "admission receives the manager context's immutable binding"
        );
        if let Some((entered, release)) = mutex_lock(&self.admission_gate).take() {
            entered
                .send(())
                .expect("test observes admission before durable commit");
            release
                .recv()
                .expect("test releases admission for durable commit");
        }
        let mut state = mutex_lock(&self.state);
        state.admission_committed = true;
        let job = match state.admitted.clone() {
            Some(job) => job,
            None => {
                let key = state
                    .admission_key
                    .clone()
                    .expect("admission test configures a durable identity");
                assert_eq!(
                    key,
                    WorkspaceSchedulerKeyV1::from_durable_identity(binding.identity()),
                    "admission cannot enqueue work for another scheduler identity"
                );
                let receipt = enqueue_for_key(&mut state, key);
                let job = receipt.job_id().clone();
                state.admitted = Some(job);
                return Ok(AdmitOutcomeV1::New(receipt));
            }
        };
        let record = state.jobs.get(&job).expect("admitted job remains present");
        Ok(match &record.terminal {
            Some(receipt) => {
                AdmitOutcomeV1::Existing(JobReceiptOrTerminalV1::TerminalReceipt(receipt.clone()))
            }
            None => {
                AdmitOutcomeV1::Existing(JobReceiptOrTerminalV1::JobReceipt(record.receipt.clone()))
            }
        })
    }

    fn execute_next(
        &self,
        workspace: &FixtureContext,
        binding: &WorkspaceBindingV1,
        _worker: WorkerIdV1,
    ) -> Result<Option<TerminalReceiptV1>, ExecutionErrorV1> {
        assert_eq!(
            workspace.binding(),
            binding,
            "claim receives the manager context's immutable binding"
        );
        let key = WorkspaceSchedulerKeyV1::from_durable_identity(binding.identity());
        let (receipt, gate, transient, panic) = {
            let mut state = mutex_lock(&self.state);
            let Some(queue) = state.queues.get_mut(&key) else {
                return Ok(None);
            };
            let Some(job) = queue.pop_front() else {
                return Ok(None);
            };
            let receipt = {
                let record = state
                    .jobs
                    .get_mut(&job)
                    .expect("queued job remains present");
                assert_eq!(
                    record.state,
                    JobStateV1::Queued,
                    "only Store-queued work is claimed"
                );
                record.state = JobStateV1::Running;
                record.receipt.clone()
            };
            if state.admitted.as_ref() == Some(&job) {
                assert!(state.admission_committed, "wake follows durable admission");
            }
            state.executed.push(job);
            let gate = match &mut state.gate {
                Gate::None => GateAction::None,
                Gate::First(first) => match first.take() {
                    Some((entered, release)) => GateAction::First(entered, release),
                    None => GateAction::None,
                },
                Gate::Every(barrier) => GateAction::Every(Arc::clone(barrier)),
            };
            let transient = std::mem::replace(&mut state.transient_once, false);
            let panic = std::mem::replace(&mut state.panic_once, false);
            (receipt, gate, transient, panic)
        };
        if panic {
            panic!("fixture injected execution panic after Store claim");
        }
        let active = self.active_executions.fetch_add(1, Ordering::SeqCst) + 1;
        self.maximum_active_executions
            .fetch_max(active, Ordering::SeqCst);
        match gate {
            GateAction::None => {}
            GateAction::First(entered, release) => {
                entered.send(()).expect("test observes claim");
                release.recv().expect("test releases claim");
            }
            GateAction::Every(barrier) => {
                barrier.wait();
            }
        }
        self.active_executions.fetch_sub(1, Ordering::SeqCst);
        if transient {
            return Err(ExecutionErrorV1::BoundaryTransient {
                operation: "fixture transient boundary",
            });
        }
        let terminal = TerminalReceiptV1::new(
            receipt.clone(),
            TerminalResultV1::Failure(DomainError::InvalidState {
                reason: "fixture terminal result",
            }),
        );
        let persisted = PersistedTerminalReceiptV1::from_terminal_receipt(&terminal);
        let mut state = mutex_lock(&self.state);
        let record = state
            .jobs
            .get_mut(receipt.job_id())
            .expect("claimed job remains present");
        record.state = JobStateV1::Failed;
        record.terminal = Some(persisted);
        Ok(Some(terminal))
    }
}

struct FixtureClock {
    monotonic: AtomicU64,
    wall: AtomicU64,
    waits: AtomicUsize,
}

impl FixtureClock {
    fn new(now: u64) -> Self {
        Self {
            monotonic: AtomicU64::new(now),
            wall: AtomicU64::new(now),
            waits: AtomicUsize::new(0),
        }
    }

    fn jump_wall(&self, now: u64) {
        self.wall.store(now, Ordering::SeqCst);
    }

    fn wall(&self) -> u64 {
        self.wall.load(Ordering::SeqCst)
    }
    fn wait_count(&self) -> usize {
        self.waits.load(Ordering::SeqCst)
    }
}

impl MonotonicClockV1 for FixtureClock {
    fn now_millis(&self) -> u64 {
        self.monotonic.load(Ordering::SeqCst)
    }
}

impl WorkerClockV1 for FixtureClock {
    fn wait_duration_until(&self, deadline: MonotonicDeadlineV1) -> Duration {
        self.waits.fetch_add(1, Ordering::SeqCst);
        self.monotonic.store(deadline.millis(), Ordering::SeqCst);
        Duration::ZERO
    }
}

type FixtureWorker = DaemonWorkerV1<FixtureContext, FixtureBoundary, FixtureClock>;
type FixtureScheduler = Arc<WorkspaceSchedulerV1<FixtureContext>>;
type FixtureRegistry = WorkspaceSchedulerRegistryV1<FixtureContext>;

fn worker(boundary: Arc<FixtureBoundary>, clock: Arc<FixtureClock>) -> FixtureWorker {
    DaemonWorkerV1::new(
        boundary,
        clock,
        WorkerIdV1::new("phase4-worker").expect("fixture worker ID is valid"),
    )
}

fn scheduler(
    registry: &FixtureRegistry,
    boundary: Arc<FixtureBoundary>,
    binding: WorkspaceBindingV1,
) -> FixtureScheduler {
    let key = WorkspaceSchedulerKeyV1::from_durable_identity(binding.identity());
    registry
        .get_or_create(key, move || FixtureContext::new(binding, boundary))
        .expect("fixture scheduler generation is valid")
}

fn identity(number: u64) -> DurableWorktreeIdentityV1 {
    DurableWorktreeIdentityV1::new(
        Sha256Digest::new(format!("sha256:{}", "a".repeat(64))).expect("fixture digest is valid"),
        WorkspaceId::new(format!("00000000-0000-0000-0000-{number:012x}"))
            .expect("fixture workspace UUID is valid"),
        Sha256Digest::new(format!("sha256:{}", "b".repeat(64))).expect("fixture digest is valid"),
    )
}

fn binding(number: u64, root: &str) -> WorkspaceBindingV1 {
    WorkspaceBindingV1::new(
        identity(number),
        podway_store::ValidatedWorkspaceRootV1::from_encoded(root)
            .expect("fixture Store root encoding is valid"),
    )
}

fn enqueue_for_key(state: &mut FixtureState, key: WorkspaceSchedulerKeyV1) -> JobReceiptV1 {
    let sequence = state.next_sequence;
    state.next_sequence += 1;
    let job_id = JobId::new(format!("00000000-0000-4000-8000-{sequence:012x}"))
        .expect("fixture job ID is valid");
    let receipt = JobReceiptV1::new(
        sequence,
        job_id.clone(),
        Sha256Digest::new(format!("sha256:{sequence:064x}")).expect("fixture digest is valid"),
    );
    state
        .queues
        .entry(key.clone())
        .or_default()
        .push_back(job_id.clone());
    state.jobs.insert(
        job_id,
        FixtureJob {
            key,
            receipt: receipt.clone(),
            state: JobStateV1::Queued,
            terminal: None,
        },
    );
    receipt
}

fn job_view(record: FixtureJob) -> JobViewV1 {
    JobViewV1::new(
        ClaimedExecutionV1::new(
            DomainCommand::WorkspaceInitialize,
            RevisionAttemptItemPreconditionsV1::new(None, None, None, None)
                .expect("empty fixture preconditions are valid"),
        ),
        record.receipt,
        record.state,
        UnixMillis::new(1),
        (record.state != JobStateV1::Queued).then_some(UnixMillis::new(2)),
        record.terminal.as_ref().map(|_| UnixMillis::new(3)),
        record.terminal,
    )
}

fn request() -> SliceRequestV1 {
    let payload: Map<String, Value> = json!({
        "selector": {
            "version": 1,
            "path_bytes_base64url": "L3dvcmt0cmVl",
            "display": "/worktree",
            "expected_uuid": "00000000-0000-0000-0000-000000000001"
        }
    })
    .as_object()
    .expect("fixture payload is an object")
    .clone();
    let envelope = RequestEnvelopeV1::new(RequestEnvelopeInputV1 {
        request_id: RequestIdV1::new("00000000-0000-4000-8000-000000000001").unwrap(),
        client: ClientInfoV1::new("worker-test", "1", 1).unwrap(),
        operation: OperationV1::Bootstrap,
        command: CommandNameV1::new("workspace.init").unwrap(),
        workspace: Some(
            WorkspaceContextV1::new(
                "/worktree",
                Some(WorkspaceId::new("00000000-0000-0000-0000-000000000001").unwrap()),
            )
            .unwrap(),
        ),
        idempotency_key: Some(ProtocolIdempotencyKeyV1::new("worker-request").unwrap()),
        preconditions: PreconditionsV1::default(),
        options: RequestOptionsV1::new(false, 0).unwrap(),
        payload,
    })
    .unwrap();
    SliceRequestV1::from_envelope(&envelope).unwrap()
}

fn idempotency_key() -> IdempotencyKeyV1 {
    IdempotencyKeyV1::new("worker-request").unwrap()
}

fn mutex_lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[test]
fn same_identity_fifo_is_serialized_on_one_manager_scheduler() {
    let boundary = Arc::new(FixtureBoundary::new());
    let worker = worker(Arc::clone(&boundary), Arc::new(FixtureClock::new(1)));
    let registry = FixtureRegistry::new();
    let binding = binding(1, "podway.unix-path/v1:2f746d702f6669666f");
    let scheduler = scheduler(&registry, Arc::clone(&boundary), binding.clone());
    let first = boundary.enqueue(&binding);
    let second = boundary.enqueue(&binding);
    let (entered, release) = boundary.first_execution_gate();
    let start = Arc::new(Barrier::new(3));
    let first_drain = {
        let worker = worker.clone();
        let scheduler = Arc::clone(&scheduler);
        let start = Arc::clone(&start);
        thread::spawn(move || {
            start.wait();
            worker.drain_workspace(scheduler)
        })
    };
    let second_drain = {
        let worker = worker.clone();
        let scheduler = Arc::clone(&scheduler);
        let start = Arc::clone(&start);
        thread::spawn(move || {
            start.wait();
            worker.drain_workspace(scheduler)
        })
    };
    start.wait();
    entered.recv().unwrap();
    assert_eq!(boundary.maximum_active_executions.load(Ordering::SeqCst), 1);
    release.send(()).unwrap();
    first_drain.join().unwrap().unwrap();
    second_drain.join().unwrap().unwrap();
    assert_eq!(
        boundary.executed(),
        vec![first.job_id().clone(), second.job_id().clone()]
    );
}

#[test]
fn aliases_reuse_the_same_scheduler_pointer_without_path_keying() {
    let boundary = Arc::new(FixtureBoundary::new());
    let registry = FixtureRegistry::new();
    let first = binding(2, "podway.unix-path/v1:2f746d702f616c6961732d61");
    let second = binding(2, "podway.unix-path/v1:2f746d702f616c6961732d62");
    let first_scheduler = scheduler(&registry, Arc::clone(&boundary), first);
    let second_scheduler = scheduler(&registry, boundary, second);
    assert!(Arc::ptr_eq(&first_scheduler, &second_scheduler));
}

#[test]
fn separate_identities_overlap_at_execution_boundary() {
    let boundary = Arc::new(FixtureBoundary::new());
    let worker = worker(Arc::clone(&boundary), Arc::new(FixtureClock::new(1)));
    let registry = FixtureRegistry::new();
    let left = binding(3, "podway.unix-path/v1:2f746d702f6c656674");
    let right = binding(4, "podway.unix-path/v1:2f746d702f7269676874");
    let left_scheduler = scheduler(&registry, Arc::clone(&boundary), left.clone());
    let right_scheduler = scheduler(&registry, Arc::clone(&boundary), right.clone());
    boundary.enqueue(&left);
    boundary.enqueue(&right);
    boundary.overlap_gate(2);
    let left_drain = {
        let worker = worker.clone();
        thread::spawn(move || worker.drain_workspace(left_scheduler))
    };
    let right_drain = thread::spawn(move || worker.drain_workspace(right_scheduler));
    left_drain.join().unwrap().unwrap();
    right_drain.join().unwrap().unwrap();
    assert_eq!(boundary.maximum_active_executions.load(Ordering::SeqCst), 2);
}

#[test]
fn admission_wake_wait_and_immutable_replay_use_the_context_store() {
    let boundary = Arc::new(FixtureBoundary::new());
    let worker = worker(Arc::clone(&boundary), Arc::new(FixtureClock::new(1)));
    let registry = FixtureRegistry::new();
    let binding = binding(5, "podway.unix-path/v1:2f746d702f61646d6974");
    boundary.configure_admission(&binding);
    let scheduler = scheduler(&registry, Arc::clone(&boundary), binding);
    let acknowledged = worker
        .submit(
            &scheduler,
            &request(),
            idempotency_key(),
            WorkerCompletionModeV1::Detached,
        )
        .unwrap();
    assert!(boundary.admission_committed());
    let job = match acknowledged.admission() {
        AdmitOutcomeV1::New(receipt) => receipt.job_id().clone(),
        _ => unreachable!(),
    };
    worker.drain_workspace(Arc::clone(&scheduler)).expect(
        "an explicit concurrent drain must join the detached drain and reach terminal state",
    );
    assert!(matches!(
        worker
            .wait_for_terminal(&scheduler, &job, MonotonicDeadlineV1::new(10_000))
            .unwrap(),
        WorkerWaitResultV1::Terminal(_)
    ));
    let replay = worker
        .submit(
            &scheduler,
            &request(),
            idempotency_key(),
            WorkerCompletionModeV1::WaitUntil(MonotonicDeadlineV1::new(10_000)),
        )
        .unwrap();
    assert!(matches!(
        replay.admission(),
        AdmitOutcomeV1::Existing(JobReceiptOrTerminalV1::TerminalReceipt(_))
    ));
    assert!(matches!(
        replay.completion(),
        Some(WorkerWaitResultV1::Terminal(_))
    ));
}

#[test]
fn deadline_rechecks_store_without_cancelling_work() {
    let boundary = Arc::new(FixtureBoundary::new());
    let worker = worker(Arc::clone(&boundary), Arc::new(FixtureClock::new(50)));
    let registry = FixtureRegistry::new();
    let binding = binding(6, "podway.unix-path/v1:2f746d702f74696d656f7574");
    let scheduler = scheduler(&registry, Arc::clone(&boundary), binding.clone());
    let job = boundary.enqueue(&binding);
    assert!(
        matches!(worker.wait_for_terminal(&scheduler, job.job_id(), MonotonicDeadlineV1::new(50)).unwrap(), WorkerWaitResultV1::TimedOut(view) if view.state() == JobStateV1::Queued)
    );
    assert_eq!(boundary.job_state(job.job_id()), JobStateV1::Queued);
}
#[test]
fn wall_clock_jump_does_not_expire_a_monotonic_mutation_wait() {
    let boundary = Arc::new(FixtureBoundary::new());
    let clock = Arc::new(FixtureClock::new(50));
    clock.jump_wall(u64::MAX);
    let worker = worker(Arc::clone(&boundary), Arc::clone(&clock));
    let registry = FixtureRegistry::new();
    let binding = binding(9, "podway.unix-path/v1:2f746d702f6d6f6e6f746f6e6963");
    let scheduler = scheduler(&registry, Arc::clone(&boundary), binding.clone());
    let job = boundary.enqueue(&binding);

    assert!(
        matches!(worker.wait_for_terminal(&scheduler, job.job_id(), MonotonicDeadlineV1::new(51)).unwrap(), WorkerWaitResultV1::TimedOut(view) if view.state() == JobStateV1::Queued)
    );
    assert_eq!(clock.wall(), u64::MAX);
    assert_eq!(
        clock.wait_count(),
        1,
        "the wall-clock jump must not bypass the monotonic wait"
    );
    assert_eq!(boundary.job_state(job.job_id()), JobStateV1::Queued);
}

#[test]
fn restart_recovery_uses_a_fresh_manager_scheduler_and_store_fifo() {
    let boundary = Arc::new(FixtureBoundary::new());
    let clock = Arc::new(FixtureClock::new(1));
    let binding = binding(7, "podway.unix-path/v1:2f746d702f7265636f766572");
    let job = boundary.enqueue(&binding);
    boundary.make_first_execution_transient();
    let first_registry = FixtureRegistry::new();
    let first_scheduler = scheduler(&first_registry, Arc::clone(&boundary), binding.clone());
    assert!(matches!(
        worker(Arc::clone(&boundary), Arc::clone(&clock)).drain_workspace(first_scheduler),
        Err(WorkerErrorV1::Execution(_))
    ));
    boundary.recover_running_to_queued();
    let restarted_registry = FixtureRegistry::new();
    let restarted_scheduler = scheduler(&restarted_registry, Arc::clone(&boundary), binding);
    let reports =
        worker(Arc::clone(&boundary), clock).drain_recovered_queues([restarted_scheduler]);
    assert_eq!(reports[0].as_ref().unwrap().terminal_job_count(), 1);
    assert_eq!(boundary.job_state(job.job_id()), JobStateV1::Failed);
}
#[test]
fn recovery_required_generation_closes_admission_and_claims_before_returning_error() {
    let boundary = Arc::new(FixtureBoundary::new());
    let worker = worker(Arc::clone(&boundary), Arc::new(FixtureClock::new(1)));
    let registry = FixtureRegistry::new();
    let binding = binding(11, "podway.unix-path/v1:2f746d702f6661696c2d636c6f736564");
    boundary.configure_admission(&binding);
    let first = boundary.enqueue(&binding);
    let second = boundary.enqueue(&binding);
    boundary.make_first_execution_transient();
    let scheduler = scheduler(&registry, Arc::clone(&boundary), binding.clone());

    assert!(matches!(
        worker.drain_workspace(Arc::clone(&scheduler)),
        Err(WorkerErrorV1::Execution(_))
    ));
    assert!(scheduler.context_snapshot().recovery_required());
    assert_eq!(boundary.executed(), vec![first.job_id().clone()]);
    assert_eq!(boundary.job_state(second.job_id()), JobStateV1::Queued);

    assert!(matches!(
        worker.submit(
            &scheduler,
            &request(),
            idempotency_key(),
            WorkerCompletionModeV1::Detached,
        ),
        Err(WorkerErrorV1::RetirementRejected)
    ));
    assert!(!boundary.admission_committed());
    assert_eq!(
        worker
            .drain_workspace(Arc::clone(&scheduler))
            .unwrap()
            .terminal_job_count(),
        0
    );
    assert_eq!(boundary.executed(), vec![first.job_id().clone()]);
    assert_eq!(boundary.job_state(second.job_id()), JobStateV1::Queued);
}

#[test]
fn detached_execution_panic_marks_recovery_and_blocks_claims_until_restart_recovery() {
    let boundary = Arc::new(FixtureBoundary::new());
    let worker = worker(Arc::clone(&boundary), Arc::new(FixtureClock::new(1)));
    let registry = FixtureRegistry::new();
    let binding = binding(12, "podway.unix-path/v1:2f746d702f70616e6963");
    boundary.configure_admission(&binding);
    let recovery_marked = boundary.panic_on_next_execution();
    let scheduler = scheduler(&registry, Arc::clone(&boundary), binding.clone());

    let first = worker
        .submit(
            &scheduler,
            &request(),
            idempotency_key(),
            WorkerCompletionModeV1::Detached,
        )
        .expect("durable admission succeeds before the detached drain panics");
    let first = match first.admission() {
        AdmitOutcomeV1::New(receipt) => receipt.job_id().clone(),
        _ => unreachable!(),
    };
    recovery_marked
        .recv()
        .expect("detached drain marks recovery before the test continues");

    assert!(scheduler.context_snapshot().recovery_required());
    assert_eq!(boundary.job_state(&first), JobStateV1::Running);
    assert_eq!(boundary.executed(), vec![first.clone()]);

    let second = boundary.enqueue(&binding);
    assert_eq!(
        worker
            .drain_workspace(Arc::clone(&scheduler))
            .expect("recovery-required generation refuses further claims")
            .terminal_job_count(),
        0
    );
    assert_eq!(boundary.job_state(second.job_id()), JobStateV1::Queued);
    assert_eq!(boundary.executed(), vec![first.clone()]);
    assert!(matches!(
        worker.submit(
            &scheduler,
            &request(),
            idempotency_key(),
            WorkerCompletionModeV1::Detached,
        ),
        Err(WorkerErrorV1::RetirementRejected)
    ));

    boundary.recover_running_to_queued();
    let restarted_registry = FixtureRegistry::new();
    let restarted_scheduler = self::scheduler(&restarted_registry, Arc::clone(&boundary), binding);
    let reports = worker.drain_recovered_queues([restarted_scheduler]);
    assert_eq!(reports[0].as_ref().unwrap().terminal_job_count(), 2);
    assert_eq!(boundary.job_state(&first), JobStateV1::Failed);
    assert_eq!(boundary.job_state(second.job_id()), JobStateV1::Failed);
}
#[test]
fn inner_drain_recovery_attempts_mark_after_stop_claims_panics() {
    let boundary = Arc::new(FixtureBoundary::new());
    let worker = worker(Arc::clone(&boundary), Arc::new(FixtureClock::new(1)));
    let registry = FixtureRegistry::new();
    let binding = binding(13, "podway.unix-path/v1:2f746d702f70616e69632d73746f70");
    let job = boundary.enqueue(&binding);
    let recovery_marked = boundary.panic_on_next_execution();
    let scheduler = scheduler(&registry, Arc::clone(&boundary), binding);
    scheduler
        .context_snapshot()
        .panic_recovery_hook_once(RecoveryHookPanic::StopClaims);

    assert!(matches!(
        worker.drain_workspace(Arc::clone(&scheduler)),
        Err(WorkerErrorV1::BackgroundPanicked)
    ));
    recovery_marked
        .recv()
        .expect("mark_recovery_required runs after stop_claims panics");

    let context = scheduler.context_snapshot();
    assert_eq!(context.recovery_hook_call_counts(), (1, 1));
    assert!(context.claims_are_stopped());
    assert!(context.recovery_required());
    assert_eq!(boundary.job_state(job.job_id()), JobStateV1::Running);
}

#[test]
fn detached_drain_recovery_attempts_stop_after_mark_panics_and_recovers_on_restart() {
    let boundary = Arc::new(FixtureBoundary::new());
    let worker = worker(Arc::clone(&boundary), Arc::new(FixtureClock::new(1)));
    let registry = FixtureRegistry::new();
    let binding = binding(14, "podway.unix-path/v1:2f746d702f70616e69632d6d61726b");
    let first = boundary.enqueue(&binding);
    let recovery_marked = boundary.panic_on_next_execution();
    let failed_scheduler = scheduler(&registry, Arc::clone(&boundary), binding.clone());
    failed_scheduler
        .context_snapshot()
        .panic_recovery_hook_once(RecoveryHookPanic::MarkRecoveryRequired);

    assert!(matches!(
        worker
            .drain_workspace_detached(Arc::clone(&failed_scheduler))
            .join(),
        Err(WorkerErrorV1::BackgroundPanicked)
    ));
    recovery_marked
        .recv()
        .expect("mark_recovery_required marks recovery before it panics");

    let context = failed_scheduler.context_snapshot();
    assert_eq!(context.recovery_hook_call_counts(), (1, 1));
    assert!(context.claims_are_stopped());
    assert!(context.recovery_required());
    assert_eq!(boundary.job_state(first.job_id()), JobStateV1::Running);

    let second = boundary.enqueue(&binding);
    assert_eq!(
        worker
            .drain_workspace(Arc::clone(&failed_scheduler))
            .expect("the failed generation remains closed to claims")
            .terminal_job_count(),
        0
    );
    assert_eq!(boundary.job_state(second.job_id()), JobStateV1::Queued);

    boundary.recover_running_to_queued();
    let restarted_registry = FixtureRegistry::new();
    let restarted_scheduler = scheduler(&restarted_registry, Arc::clone(&boundary), binding);
    let reports = worker.drain_recovered_queues([restarted_scheduler]);
    assert_eq!(reports[0].as_ref().unwrap().terminal_job_count(), 2);
    assert_eq!(boundary.job_state(first.job_id()), JobStateV1::Failed);
    assert_eq!(boundary.job_state(second.job_id()), JobStateV1::Failed);
}
#[test]
fn graceful_retirement_waits_for_claimed_work_and_stops_future_claims() {
    let boundary = Arc::new(FixtureBoundary::new());
    let worker = worker(Arc::clone(&boundary), Arc::new(FixtureClock::new(1)));
    let registry = FixtureRegistry::new();
    let binding = binding(8, "podway.unix-path/v1:2f746d702f677261636566756c");
    let scheduler = scheduler(&registry, Arc::clone(&boundary), binding.clone());
    let job = boundary.enqueue(&binding);
    let (entered, release) = boundary.first_execution_gate();
    let drain = worker.drain_workspace_detached(Arc::clone(&scheduler));
    entered.recv().unwrap();
    let retirement = {
        let worker = worker.clone();
        let registry = registry.clone();
        let scheduler = Arc::clone(&scheduler);
        thread::spawn(move || worker.retire_workspace(&registry, &scheduler))
    };
    release.send(()).unwrap();
    drain.join().unwrap();
    retirement.join().unwrap().unwrap();
    assert_eq!(boundary.job_state(job.job_id()), JobStateV1::Failed);
}
#[test]
fn admission_racing_maintenance_retirement_is_drained_and_old_generation_cannot_claim_future_work()
{
    let boundary = Arc::new(FixtureBoundary::new());
    let worker = worker(Arc::clone(&boundary), Arc::new(FixtureClock::new(1)));
    let registry = FixtureRegistry::new();
    let binding = binding(10, "podway.unix-path/v1:2f746d702f61646d69742d726574697265");
    boundary.configure_admission(&binding);
    let retiring = scheduler(&registry, Arc::clone(&boundary), binding.clone());
    let (admission_entered, admission_release) = boundary.gate_admission();

    let submit = {
        let worker = worker.clone();
        let scheduler = Arc::clone(&retiring);
        thread::spawn(move || {
            worker.submit(
                &scheduler,
                &request(),
                idempotency_key(),
                WorkerCompletionModeV1::Detached,
            )
        })
    };
    admission_entered
        .recv()
        .expect("admission holds the scheduler serialization gate");
    let (close_entered_sender, close_entered) = mpsc::channel();
    let (close_release, close_release_receiver) = mpsc::channel();
    let retirement = {
        let worker = worker.clone();
        let registry = registry.clone();
        let scheduler = Arc::clone(&retiring);
        thread::spawn(move || {
            worker.retire_workspace_for_maintenance(&registry, &scheduler, |_| {
                close_entered_sender
                    .send(())
                    .expect("test observes retirement after queue drain");
                close_release_receiver
                    .recv()
                    .expect("test releases retirement close");
                Ok(())
            })
        })
    };

    admission_release
        .send(())
        .expect("admission releases for durable commit");
    let admitted = submit
        .join()
        .expect("admission thread does not panic")
        .expect("committed admission remains acknowledged");
    let admitted_job = match admitted.admission() {
        AdmitOutcomeV1::New(receipt) => receipt.job_id().clone(),
        _ => panic!("gated admission creates exactly one durable job"),
    };
    close_entered
        .recv()
        .expect("retirement drains after the admission commits");
    close_release.send(()).expect("retirement close completes");
    retirement
        .join()
        .expect("retirement thread does not panic")
        .expect("retirement drains the acknowledged admission");
    assert_eq!(boundary.job_state(&admitted_job), JobStateV1::Failed);

    let future = boundary.enqueue(&binding);
    assert_eq!(
        worker
            .drain_workspace(Arc::clone(&retiring))
            .expect("old retirement generation is closed")
            .terminal_job_count(),
        0,
        "the old generation cannot claim work admitted after it retired"
    );
    assert_eq!(boundary.job_state(future.job_id()), JobStateV1::Queued);

    let recovery = scheduler(&registry, Arc::clone(&boundary), binding);
    assert_eq!(
        recovery.generation().get(),
        retiring.generation().get() + 1,
        "future work is claimed by the recovery generation"
    );
    assert_eq!(
        worker
            .drain_workspace(recovery)
            .expect("recovery generation drains future work")
            .terminal_job_count(),
        1
    );
    assert_eq!(boundary.job_state(future.job_id()), JobStateV1::Failed);
}
#[test]
fn retirement_closes_exact_generation_and_retry_does_not_reopen_it() {
    let boundary = Arc::new(FixtureBoundary::new());
    let worker = worker(Arc::clone(&boundary), Arc::new(FixtureClock::new(1)));
    let registry = FixtureRegistry::new();
    let binding = binding(8, "podway.unix-path/v1:2f746d702f726574697265");
    let retiring_scheduler = scheduler(&registry, Arc::clone(&boundary), binding.clone());
    let failure = worker
        .retire_workspace_with(&registry, &retiring_scheduler, |_| {
            Err(WorkerErrorV1::RetirementRejected)
        })
        .expect_err("failed close retains a typed retry");
    assert!(std::error::Error::source(&failure).is_some());
    let retry = match failure {
        WorkerRetirementErrorV1::CloseFailed { retry, .. } => retry,
        _ => panic!("failed close must retain a typed retry"),
    };
    retry.retry_with(|_| Ok(())).unwrap();
    let replacement = scheduler(&registry, boundary, binding);
    assert_eq!(
        replacement.generation().get(),
        retiring_scheduler.generation().get() + 1
    );
}
#[test]
fn maintenance_retirement_drains_stops_claims_closes_store_then_runs_callback() {
    let boundary = Arc::new(FixtureBoundary::new());
    let worker = worker(Arc::clone(&boundary), Arc::new(FixtureClock::new(1)));
    let registry = FixtureRegistry::new();
    let binding = binding(15, "podway.unix-path/v1:2f746d702f6d61696e74656e616e6365");
    let scheduler = scheduler(&registry, Arc::clone(&boundary), binding.clone());
    let first = boundary.enqueue(&binding);
    let second = boundary.enqueue(&binding);
    let context = scheduler.context_snapshot();
    let callback_context = Arc::clone(&context);
    let callback_boundary = Arc::clone(&boundary);
    let callback_count = Arc::new(AtomicUsize::new(0));
    let observed_callback_count = Arc::clone(&callback_count);

    worker
        .retire_workspace_for_maintenance(&registry, &scheduler, move |_| {
            assert_eq!(
                callback_boundary.executed(),
                vec![first.job_id().clone(), second.job_id().clone()],
                "maintenance starts only after all earlier FIFO work reaches terminal state"
            );
            assert!(callback_context.claims_are_stopped());
            assert!(callback_context.maintenance_store_is_closed());
            observed_callback_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .expect("maintenance retirement closes the Store before the callback");

    assert_eq!(callback_count.load(Ordering::SeqCst), 1);
    assert_eq!(context.maintenance_close_call_count(), 1);
    assert!(matches!(
        worker.submit(
            &scheduler,
            &request(),
            idempotency_key(),
            WorkerCompletionModeV1::Detached,
        ),
        Err(WorkerErrorV1::RetirementRejected)
    ));
}

#[test]
fn failed_maintenance_store_close_is_cached_and_keeps_the_generation_fail_closed() {
    let boundary = Arc::new(FixtureBoundary::new());
    let worker = worker(Arc::clone(&boundary), Arc::new(FixtureClock::new(1)));
    let registry = FixtureRegistry::new();
    let binding = binding(
        16,
        "podway.unix-path/v1:2f746d702f636c6f73652d6661696c757265",
    );
    let scheduler = scheduler(&registry, Arc::clone(&boundary), binding.clone());
    let context = scheduler.context_snapshot();
    context.fail_maintenance_store_close(StoreErrorV1::StorageUnavailableV1 {
        reason: podway_store::StoreUnavailableReasonV1::Busy,
    });
    let callback_count = Arc::new(AtomicUsize::new(0));
    let first_callback_count = Arc::clone(&callback_count);

    let failure = worker
        .retire_workspace_for_maintenance(&registry, &scheduler, move |_| {
            first_callback_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        })
        .expect_err("a failed Store close keeps retirement retryable");
    assert!(context.claims_are_stopped());
    assert!(context.maintenance_store_is_closed());
    assert_eq!(context.maintenance_close_call_count(), 1);
    assert_eq!(callback_count.load(Ordering::SeqCst), 0);

    let retry = match failure {
        WorkerRetirementErrorV1::CloseFailed { retry, .. } => retry,
        _ => panic!("maintenance close failure retains a typed retry"),
    };
    let retry_callback_count = Arc::clone(&callback_count);
    assert!(matches!(
        retry.retry_with_maintenance(move |_| {
            retry_callback_count.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }),
        Err(WorkerRetirementErrorV1::CloseFailed {
            source,
            ..
        }) if matches!(*source, WorkerErrorV1::Store(StoreErrorV1::StorageUnavailableV1 {
            reason: podway_store::StoreUnavailableReasonV1::Busy
        }))
    ));
    assert_eq!(
        context.maintenance_close_call_count(),
        2,
        "a retry reuses the cached close failure instead of reopening the Store"
    );
    assert_eq!(callback_count.load(Ordering::SeqCst), 0);
    assert!(registry.get_active(scheduler.key()).is_none());
    assert!(matches!(
        worker.submit(
            &scheduler,
            &request(),
            idempotency_key(),
            WorkerCompletionModeV1::Detached,
        ),
        Err(WorkerErrorV1::RetirementRejected)
    ));
}
