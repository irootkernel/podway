//! Focused daemon authoritative-read contracts using only Store and wait-boundary doubles.

use std::{
    collections::VecDeque,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use podway_config::{ProcedureFormatV1, ProcedureWarningPolicyV1, parse_procedure_v1};
use podway_core::{
    AttemptId, BlockSessionV1, BlockerId, CommandContextV1, CompleteSessionV1, DomainCommand,
    ItemId, ItemMutationPreconditionsV1, ItemValueV1, JobId, ProcedureSnapshotId,
    ProcedureSourceLabelV1, RetrySessionV1, ReturnSessionV1, Revision, SessionAggregateV1,
    SessionCommandV1, SessionId, SessionState, SetItemV1, Sha256Digest, UnixMillis, WorkspaceId,
    WorkspaceState, apply_transition_v1,
};
use podway_daemon::read_service::{
    AuthoritativeReadServiceV1, MonotonicClockV1, MonotonicDeadlineV1, ReadNotificationErrorV1,
    ReadNotificationV1, ReadNotificationVersionV1, ReadServiceErrorV1, ReadWaitOutcomeV1,
    ReadWaitV1,
};
use podway_protocol::StageStatusResultV1;
use podway_store::{
    AdmitOutcomeV1, AdmitRequestV1, CancelOutcomeV1, ClaimTokenV1, ClaimedExecutionV1,
    ClaimedJobV1, DurableWorktreeIdentityV1, JobIdV1, JobListQueryV1, JobReceiptV1, JobStateV1,
    JobViewV1, RevisionAttemptItemPreconditionsV1, StateTransitionV1, StoreContractV1,
    StoreErrorV1, StoreReadContractV1, StoreUnavailableReasonV1, TerminalReceiptV1,
    TerminalResultV1, WorkerIdV1, WorkspaceViewV1,
};

fn digest(nibble: char) -> Sha256Digest {
    Sha256Digest::new(format!("sha256:{}", nibble.to_string().repeat(64)))
        .expect("fixture digest is valid")
}

fn identity() -> DurableWorktreeIdentityV1 {
    DurableWorktreeIdentityV1::new(
        digest('a'),
        WorkspaceId::new("00000000-0000-4000-8000-000000000001")
            .expect("fixture workspace UUID is valid"),
        digest('b'),
    )
}

fn attempt(number: u64) -> AttemptId {
    AttemptId::new(format!("00000000-0000-4000-8000-{number:012x}"))
        .expect("fixture attempt UUID is valid")
}

fn job(number: u64) -> JobId {
    JobId::new(format!("00000000-0000-4000-8000-{number:012x}")).expect("fixture job UUID is valid")
}

fn snapshot() -> podway_core::ProcedureSnapshotV1 {
    parse_procedure_v1(
        r#"{
            "schema": "podway.procedure/v1",
            "id": "fixture",
            "version": "1",
            "name": "Fixture",
            "stages": [
                {
                    "id": "first",
                    "title": "First",
                    "instructions": ["Record the optional note."],
                    "items": [
                        {
                            "type": "text",
                            "id": "note",
                            "prompt": "Optional note",
                            "required": false,
                            "min_length": 1,
                            "max_length": 100,
                            "multiline": false
                        }
                    ]
                },
                {
                    "id": "second",
                    "title": "Second",
                    "instructions": ["Confirm the second stage."],
                    "items": [
                        {
                            "type": "confirm",
                            "id": "confirmed",
                            "prompt": "Confirmed",
                            "required": true
                        }
                    ]
                }
            ],
            "rework": {"allow_return_to": "any_previous"}
        }"#,
        ProcedureFormatV1::Json,
    )
    .expect("fixture procedure parses")
    .into_snapshot_v1(
        ProcedureSnapshotId::new("00000000-0000-4000-8000-000000000010")
            .expect("fixture snapshot UUID is valid"),
        ProcedureSourceLabelV1::file("fixture").expect("fixture source label is valid"),
        UnixMillis::new(1_000),
        ProcedureWarningPolicyV1::Accept,
    )
    .expect("fixture snapshot is valid")
}

fn aggregate() -> SessionAggregateV1 {
    aggregate_with_session("00000000-0000-4000-8000-000000000011")
}

fn aggregate_with_session(session_id: &str) -> SessionAggregateV1 {
    SessionAggregateV1::start(
        SessionId::new(session_id).expect("fixture session UUID is valid"),
        "Fixture task",
        snapshot(),
        attempt(1),
        UnixMillis::new(2_000),
    )
    .expect("fixture aggregate is valid")
}

fn transition(
    aggregate: &SessionAggregateV1,
    command: SessionCommandV1,
    now: u64,
) -> SessionAggregateV1 {
    apply_transition_v1(
        Some(aggregate),
        &command,
        CommandContextV1 {
            expected_revision: aggregate.revision(),
            now: UnixMillis::new(now),
        },
    )
    .expect("fixture transition succeeds")
    .next_aggregate()
    .cloned()
    .expect("fixture transition retains a session")
}

fn workspace_view(
    aggregate: SessionAggregateV1,
    queued_job_count: u32,
    running_job_id: Option<JobIdV1>,
    latest_workspace_sequence: u64,
) -> WorkspaceViewV1 {
    let identity = identity();
    let session = SessionState::new(
        aggregate.session_id().clone(),
        aggregate.lifecycle(),
        aggregate.revision(),
        aggregate.active_stage_id().cloned(),
        aggregate.active_attempt_id().cloned(),
    )
    .expect("fixture cursor is valid");
    let state = WorkspaceState::new(
        identity.workspace_uuid().clone(),
        aggregate.revision(),
        Some(session),
    )
    .expect("fixture workspace state is valid");
    WorkspaceViewV1::new_coherent(
        identity,
        state,
        Some(aggregate),
        queued_job_count,
        running_job_id,
        latest_workspace_sequence,
        UnixMillis::new(3_000),
    )
}

fn job_view(job_id: JobIdV1, state: JobStateV1) -> JobViewV1 {
    JobViewV1::new(
        ClaimedExecutionV1::new(
            DomainCommand::WorkspaceInitialize,
            RevisionAttemptItemPreconditionsV1::new(None, None, None, None)
                .expect("fixture job preconditions are valid"),
        ),
        JobReceiptV1::new(7, job_id, digest('c')),
        state,
        UnixMillis::new(2_500),
        None,
        None,
        None,
    )
}

#[derive(Clone)]
struct FakeStore {
    state: Arc<Mutex<FakeStoreState>>,
    workspace_reads: Arc<AtomicUsize>,
    job_reads: Arc<AtomicUsize>,
}

struct FakeStoreState {
    view: WorkspaceViewV1,
    job: Option<JobViewV1>,
}

impl FakeStore {
    fn new(view: WorkspaceViewV1, job: Option<JobViewV1>) -> Self {
        Self {
            state: Arc::new(Mutex::new(FakeStoreState { view, job })),
            workspace_reads: Arc::new(AtomicUsize::new(0)),
            job_reads: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn set_idle(&self) {
        let mut state = self
            .state
            .lock()
            .expect("fixture Store lock is not poisoned");
        state.view = workspace_view(
            state
                .view
                .current_session()
                .expect("fixture session exists")
                .clone(),
            0,
            None,
            state.view.latest_workspace_sequence() + 1,
        );
    }

    fn set_terminal_job(&self) {
        let mut state = self
            .state
            .lock()
            .expect("fixture Store lock is not poisoned");
        let job_id = state
            .job
            .as_ref()
            .expect("fixture job exists")
            .job()
            .job_id()
            .clone();
        state.job = Some(job_view(job_id, JobStateV1::Succeeded));
    }

    fn replace_session(&self, idle: bool) {
        let mut state = self
            .state
            .lock()
            .expect("fixture Store lock is not poisoned");
        let queued_job_count = if idle {
            0
        } else {
            state.view.queued_job_count()
        };
        let running_job_id = if idle {
            None
        } else {
            state.view.running_job_id().cloned()
        };
        let latest_workspace_sequence = state.view.latest_workspace_sequence() + 1;
        state.view = workspace_view(
            aggregate_with_session("00000000-0000-4000-8000-000000000099"),
            queued_job_count,
            running_job_id,
            latest_workspace_sequence,
        );
    }
}

fn unavailable() -> StoreErrorV1 {
    StoreErrorV1::StorageUnavailableV1 {
        reason: StoreUnavailableReasonV1::Recovery,
    }
}

impl StoreContractV1 for FakeStore {
    fn admit(
        &self,
        _identity: &DurableWorktreeIdentityV1,
        _request: AdmitRequestV1,
    ) -> Result<AdmitOutcomeV1, StoreErrorV1> {
        Err(unavailable())
    }

    fn claim_next(
        &self,
        _identity: &DurableWorktreeIdentityV1,
        _worker: WorkerIdV1,
        _now: UnixMillis,
    ) -> Result<Option<ClaimedJobV1>, StoreErrorV1> {
        Err(unavailable())
    }

    fn cancel_before_claim(
        &self,
        _identity: &DurableWorktreeIdentityV1,
        _job: JobIdV1,
        _expected_job_revision: Revision,
        _now: UnixMillis,
    ) -> Result<CancelOutcomeV1, StoreErrorV1> {
        Err(unavailable())
    }

    fn commit_terminal(
        &self,
        _claim: ClaimTokenV1,
        _expected_workspace_revision: Revision,
        _transition: Option<StateTransitionV1>,
        _result: TerminalResultV1,
        _now: UnixMillis,
    ) -> Result<TerminalReceiptV1, StoreErrorV1> {
        Err(unavailable())
    }

    fn read_workspace_view(
        &self,
        _identity: &DurableWorktreeIdentityV1,
    ) -> Result<WorkspaceViewV1, StoreErrorV1> {
        self.workspace_reads.fetch_add(1, Ordering::SeqCst);
        Ok(self
            .state
            .lock()
            .expect("fixture Store lock is not poisoned")
            .view
            .clone())
    }
}

impl StoreReadContractV1 for FakeStore {
    fn read_session_aggregate(
        &self,
        _identity: &DurableWorktreeIdentityV1,
    ) -> Result<Option<SessionAggregateV1>, StoreErrorV1> {
        Ok(self
            .state
            .lock()
            .expect("fixture Store lock is not poisoned")
            .view
            .current_session()
            .cloned())
    }

    fn read_job(
        &self,
        _identity: &DurableWorktreeIdentityV1,
        _job: &JobIdV1,
    ) -> Result<Option<JobViewV1>, StoreErrorV1> {
        self.job_reads.fetch_add(1, Ordering::SeqCst);
        Ok(self
            .state
            .lock()
            .expect("fixture Store lock is not poisoned")
            .job
            .clone())
    }

    fn list_jobs(
        &self,
        _identity: &DurableWorktreeIdentityV1,
        _query: JobListQueryV1,
    ) -> Result<Vec<JobViewV1>, StoreErrorV1> {
        Ok(self
            .state
            .lock()
            .expect("fixture Store lock is not poisoned")
            .job
            .clone()
            .into_iter()
            .collect())
    }
}

#[derive(Clone, Copy)]
struct FixedClock(u64);

impl MonotonicClockV1 for FixedClock {
    fn now_millis(&self) -> u64 {
        self.0
    }
}

enum NotificationAction {
    Notify,
    MakeIdleThenNotify,
    MakeTerminalThenNotify,
    ReplaceSessionAndMakeIdleThenNotify,
    ReplaceSessionAndMakeTerminalThenNotify,
    TimeOut,
}

struct ScriptedNotifications {
    store: FakeStore,
    actions: Mutex<VecDeque<NotificationAction>>,
}

impl ScriptedNotifications {
    fn new(store: FakeStore, actions: impl IntoIterator<Item = NotificationAction>) -> Self {
        Self {
            store,
            actions: Mutex::new(actions.into_iter().collect()),
        }
    }
}

impl ReadNotificationV1 for ScriptedNotifications {
    fn observe(
        &self,
        _identity: &DurableWorktreeIdentityV1,
    ) -> Result<ReadNotificationVersionV1, ReadNotificationErrorV1> {
        Ok(ReadNotificationVersionV1::INITIAL)
    }

    fn wait_for_change(
        &self,
        _identity: &DurableWorktreeIdentityV1,
        _observed: ReadNotificationVersionV1,
        _deadline: MonotonicDeadlineV1,
    ) -> Result<ReadWaitOutcomeV1, ReadNotificationErrorV1> {
        match self
            .actions
            .lock()
            .expect("fixture notification lock is not poisoned")
            .pop_front()
            .unwrap_or(NotificationAction::TimeOut)
        {
            NotificationAction::Notify => Ok(ReadWaitOutcomeV1::Notified),
            NotificationAction::MakeIdleThenNotify => {
                self.store.set_idle();
                Ok(ReadWaitOutcomeV1::Notified)
            }
            NotificationAction::MakeTerminalThenNotify => {
                self.store.set_terminal_job();
                Ok(ReadWaitOutcomeV1::Notified)
            }
            NotificationAction::ReplaceSessionAndMakeIdleThenNotify => {
                self.store.replace_session(true);
                Ok(ReadWaitOutcomeV1::Notified)
            }
            NotificationAction::ReplaceSessionAndMakeTerminalThenNotify => {
                self.store.replace_session(false);
                self.store.set_terminal_job();
                Ok(ReadWaitOutcomeV1::Notified)
            }
            NotificationAction::TimeOut => Ok(ReadWaitOutcomeV1::TimedOut),
        }
    }
}

fn fixture_session_id() -> SessionId {
    SessionId::new("00000000-0000-4000-8000-000000000011").expect("fixture session UUID is valid")
}

fn replacement_session_id() -> SessionId {
    SessionId::new("00000000-0000-4000-8000-000000000099")
        .expect("replacement session UUID is valid")
}

#[test]
fn guarded_immediate_read_rejects_a_replaced_session() {
    let store = FakeStore::new(workspace_view(aggregate(), 0, None, 8), None);
    store.replace_session(true);
    let notifications = ScriptedNotifications::new(store.clone(), []);
    let service = AuthoritativeReadServiceV1::new(store, notifications, FixedClock(0));

    let result = service.status_guarded(
        &identity(),
        ReadWaitV1::Immediate,
        false,
        Some(&fixture_session_id()),
    );

    assert_eq!(
        result,
        Err(ReadServiceErrorV1::SessionIdentityMismatch {
            expected: fixture_session_id(),
            actual: Some(replacement_session_id()),
        }),
    );
}

#[test]
fn guarded_idle_wait_rejects_session_replacement_after_notification() {
    let running_job = job(28);
    let store = FakeStore::new(
        workspace_view(aggregate(), 1, Some(running_job.clone()), 9),
        Some(job_view(running_job, JobStateV1::Running)),
    );
    let notifications = ScriptedNotifications::new(
        store.clone(),
        [NotificationAction::ReplaceSessionAndMakeIdleThenNotify],
    );
    let service = AuthoritativeReadServiceV1::new(store, notifications, FixedClock(0));

    let result = service.status_guarded(
        &identity(),
        ReadWaitV1::IdleUntil(MonotonicDeadlineV1::new(10)),
        false,
        Some(&fixture_session_id()),
    );

    assert_eq!(
        result,
        Err(ReadServiceErrorV1::SessionIdentityMismatch {
            expected: fixture_session_id(),
            actual: Some(replacement_session_id()),
        }),
    );
}

#[test]
fn guarded_after_job_wait_rejects_session_replacement_after_notification() {
    let target_job = job(29);
    let store = FakeStore::new(
        workspace_view(aggregate(), 0, None, 10),
        Some(job_view(target_job.clone(), JobStateV1::Running)),
    );
    let notifications = ScriptedNotifications::new(
        store.clone(),
        [NotificationAction::ReplaceSessionAndMakeTerminalThenNotify],
    );
    let service = AuthoritativeReadServiceV1::new(store, notifications, FixedClock(0));

    let result = service.next_guarded(
        &identity(),
        ReadWaitV1::AfterJobUntil {
            job_id: target_job,
            deadline: MonotonicDeadlineV1::new(10),
        },
        Some(&fixture_session_id()),
    );

    assert_eq!(
        result,
        Err(ReadServiceErrorV1::SessionIdentityMismatch {
            expected: fixture_session_id(),
            actual: Some(replacement_session_id()),
        }),
    );
}

#[test]
fn status_projects_one_current_attempt_and_typed_item_value() {
    let aggregate = aggregate();
    let aggregate = transition(
        &aggregate,
        SessionCommandV1::Set(SetItemV1 {
            item_id: ItemId::new("note").expect("fixture item ID is valid"),
            value: ItemValueV1::text("recorded"),
            preconditions: ItemMutationPreconditionsV1 {
                expected_attempt_id: attempt(1),
                expected_item_revision: Revision::ZERO,
            },
        }),
        2_001,
    );
    let aggregate = transition(
        &aggregate,
        SessionCommandV1::Block(BlockSessionV1 {
            expected_attempt_id: attempt(1),
            blocker_id: BlockerId::new("00000000-0000-4000-8000-000000000012")
                .expect("fixture blocker UUID is valid"),
            reason: "review required".to_owned(),
        }),
        2_002,
    );
    let store = FakeStore::new(workspace_view(aggregate, 0, None, 9), None);
    let notifications = ScriptedNotifications::new(store.clone(), []);
    let service = AuthoritativeReadServiceV1::new(store, notifications, FixedClock(0));

    let status = service
        .status(&identity(), ReadWaitV1::Immediate)
        .expect("immediate status succeeds");

    assert_eq!(
        status
            .current
            .as_ref()
            .map(|current| current.attempt_number),
        Some(1)
    );
    assert!(
        status
            .current
            .as_ref()
            .is_some_and(|current| current.blocked)
    );
    assert_eq!(status.items.len(), 1);
    assert_eq!(status.items[0].revision, Revision::new(1));
    assert!(status.items[0].satisfied);
    assert_eq!(status.items[0].value, serde_json::json!("recorded"));
    assert_eq!(status.blockers.len(), 1);
    assert_eq!(status.blockers[0].reason, "review required");
    assert_eq!(
        status
            .stages
            .iter()
            .filter(|stage| stage.status == StageStatusResultV1::Blocked)
            .count(),
        1
    );
}

#[test]
fn retry_clears_active_item_projection_and_return_marks_reached_stage_redo() {
    let aggregate = aggregate();
    let aggregate = transition(
        &aggregate,
        SessionCommandV1::Set(SetItemV1 {
            item_id: ItemId::new("note").expect("fixture item ID is valid"),
            value: ItemValueV1::text("discard on retry"),
            preconditions: ItemMutationPreconditionsV1 {
                expected_attempt_id: attempt(1),
                expected_item_revision: Revision::ZERO,
            },
        }),
        2_001,
    );
    let aggregate = transition(
        &aggregate,
        SessionCommandV1::Retry(RetrySessionV1 {
            expected_attempt_id: attempt(1),
            reason: "start over".to_owned(),
            next_attempt_id: attempt(2),
        }),
        2_002,
    );
    let aggregate = transition(
        &aggregate,
        SessionCommandV1::Complete(CompleteSessionV1 {
            expected_attempt_id: attempt(2),
            next_attempt_id: Some(attempt(3)),
            local_artifact_verifications: Vec::new(),
        }),
        2_003,
    );
    let aggregate = transition(
        &aggregate,
        SessionCommandV1::Return(ReturnSessionV1 {
            expected_attempt_id: attempt(3),
            destination_stage_id: podway_core::StageId::new("first")
                .expect("fixture stage ID is valid"),
            reason: "rework first stage".to_owned(),
            destination_attempt_id: attempt(4),
        }),
        2_004,
    );
    let store = FakeStore::new(workspace_view(aggregate, 0, None, 10), None);
    let notifications = ScriptedNotifications::new(store.clone(), []);
    let service = AuthoritativeReadServiceV1::new(store, notifications, FixedClock(0));

    let status = service
        .status(&identity(), ReadWaitV1::Immediate)
        .expect("retry/return status succeeds");
    let next = service
        .next(&identity(), ReadWaitV1::Immediate)
        .expect("retry/return next succeeds");

    assert_eq!(
        status
            .current
            .as_ref()
            .map(|current| current.attempt_number),
        Some(3)
    );
    assert_eq!(status.items[0].revision, Revision::ZERO);
    assert_eq!(status.items[0].value, serde_json::Value::Null);
    assert_eq!(status.stages[1].status, StageStatusResultV1::Redo);
    assert_eq!(
        next.stage.as_ref().map(|stage| stage.id.as_str()),
        Some("first")
    );
    assert_eq!(
        next.allowed_actions.return_to,
        Vec::<podway_core::StageId>::new()
    );
}

#[test]
fn status_reports_queue_pending_running_job_and_latest_sequence() {
    let running_job = job(30);
    let store = FakeStore::new(
        workspace_view(aggregate(), 2, Some(running_job.clone()), 41),
        Some(job_view(running_job.clone(), JobStateV1::Running)),
    );
    let notifications = ScriptedNotifications::new(store.clone(), []);
    let service = AuthoritativeReadServiceV1::new(store, notifications, FixedClock(0));

    let status = service
        .status(&identity(), ReadWaitV1::Immediate)
        .expect("immediate status succeeds");

    assert!(status.queue.pending_mutations);
    assert_eq!(status.queue.queued_count, 2);
    assert_eq!(status.queue.running_job_id, Some(running_job));
    assert_eq!(status.queue.latest_workspace_sequence, 41);
}

#[test]
fn wait_for_idle_times_out_when_the_authoritative_queue_stays_pending() {
    let running_job = job(31);
    let store = FakeStore::new(
        workspace_view(aggregate(), 1, Some(running_job.clone()), 42),
        Some(job_view(running_job, JobStateV1::Running)),
    );
    let notifications = ScriptedNotifications::new(store.clone(), [NotificationAction::TimeOut]);
    let service = AuthoritativeReadServiceV1::new(store.clone(), notifications, FixedClock(0));

    let result = service.status(
        &identity(),
        ReadWaitV1::IdleUntil(MonotonicDeadlineV1::new(10)),
    );

    assert_eq!(result, Err(ReadServiceErrorV1::WaitTimedOut));
    assert!(store.workspace_reads.load(Ordering::SeqCst) >= 2);
}

#[test]
fn spurious_idle_notification_rechecks_store_before_returning() {
    let running_job = job(32);
    let store = FakeStore::new(
        workspace_view(aggregate(), 1, Some(running_job.clone()), 43),
        Some(job_view(running_job, JobStateV1::Running)),
    );
    let notifications = ScriptedNotifications::new(
        store.clone(),
        [
            NotificationAction::Notify,
            NotificationAction::MakeIdleThenNotify,
        ],
    );
    let service = AuthoritativeReadServiceV1::new(store.clone(), notifications, FixedClock(0));

    let status = service
        .status(
            &identity(),
            ReadWaitV1::IdleUntil(MonotonicDeadlineV1::new(10)),
        )
        .expect("idle wait returns only after Store reports no pending mutations");

    assert!(!status.queue.pending_mutations);
    assert_eq!(status.queue.queued_count, 0);
    assert!(status.queue.running_job_id.is_none());
    assert_eq!(status.queue.latest_workspace_sequence, 44);
    assert!(store.workspace_reads.load(Ordering::SeqCst) >= 3);
}

#[test]
fn after_job_wait_rechecks_terminal_state_after_spurious_notification() {
    let target_job = job(33);
    let store = FakeStore::new(
        workspace_view(aggregate(), 0, None, 44),
        Some(job_view(target_job.clone(), JobStateV1::Running)),
    );
    let notifications = ScriptedNotifications::new(
        store.clone(),
        [
            NotificationAction::Notify,
            NotificationAction::MakeTerminalThenNotify,
        ],
    );
    let service = AuthoritativeReadServiceV1::new(store.clone(), notifications, FixedClock(0));

    let next = service
        .next(
            &identity(),
            ReadWaitV1::AfterJobUntil {
                job_id: target_job,
                deadline: MonotonicDeadlineV1::new(10),
            },
        )
        .expect("after-job read returns only after a terminal Store recheck");

    assert_eq!(
        next.stage.as_ref().map(|stage| stage.id.as_str()),
        Some("first")
    );
    assert!(store.job_reads.load(Ordering::SeqCst) >= 3);
    assert!(store.workspace_reads.load(Ordering::SeqCst) >= 1);
}
