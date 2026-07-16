include!("phase4_execution.rs");
use podway_core::DomainResult;
use podway_daemon::execution::ResetAllPreparationOutcomeV1;
use podway_protocol::canonical_reset_all_identity_v1;
use sha2::{Digest as _, Sha256};

#[test]
fn g006_start_replace_skip_cancel_and_reset_are_durable_transitions() {
    let mut harness = Harness::new();
    assert_success(&harness.start());
    let replaced_session_id = harness
        .store
        .current_session()
        .unwrap()
        .session_id()
        .clone();
    let replace_preconditions = harness.session_identity_preconditions();
    let replace = harness.submit(
        "session.start_replace",
        json!({
            "selector": selector_json(),
            "preset": "sw-dev",
            "task_title": "Replacement",
            "confirmed": true,
        }),
        replace_preconditions,
    );
    assert_success(&replace);
    assert!(matches!(
        replace.result(),
        TerminalResultV1::Success(DomainResult::SessionChanged { .. })
    ));
    assert_ne!(
        harness.store.current_session().unwrap().session_id(),
        &replaced_session_id
    );

    let skip_preconditions = harness.session_preconditions();
    assert_success(&harness.submit(
        "session.skip",
        json!({"selector": selector_json(), "reason": "not applicable"}),
        skip_preconditions,
    ));
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

    let cancel_preconditions = harness.session_preconditions();
    assert_success(&harness.submit(
        "session.cancel",
        json!({"selector": selector_json(), "reason": "stop now"}),
        cancel_preconditions,
    ));
    let reset_preconditions = harness.session_identity_preconditions();
    assert_success(&harness.submit(
        "session.reset",
        json!({"selector": selector_json(), "confirmed": true, "dry_run": false}),
        reset_preconditions,
    ));
    assert!(harness.store.current_session().is_none());
}

#[test]
fn g006_start_replace_replaces_a_revision_above_one_with_a_fresh_session_and_replays() {
    let mut harness = Harness::new();
    assert_success(&harness.start());
    assert_success(&harness.check("confirm"));
    let previous = harness.store.current_session().unwrap();
    let replacement_request = slice_request(
        "session.start_replace",
        json!({
            "selector": selector_json(),
            "preset": "sw-dev",
            "task_title": "Fresh replacement",
            "confirmed": true,
        }),
        harness.session_identity_preconditions(),
        9_710,
    );
    let key = IdempotencyKeyV1::new("fresh-replacement-replay").unwrap();

    assert!(matches!(
        harness.engine.admit(&replacement_request, key.clone()),
        Ok(AdmitOutcomeV1::New(_))
    ));
    let receipt = harness
        .engine
        .execute_next(
            &harness.binding,
            WorkerIdV1::new("fresh-replacement-worker").unwrap(),
        )
        .unwrap()
        .unwrap();
    assert_success(&receipt);
    match receipt.result() {
        TerminalResultV1::Success(DomainResult::SessionChanged {
            session_id,
            revision_before,
            revision_after,
            changed,
        }) => {
            assert_ne!(session_id, previous.session_id());
            assert_eq!(*revision_before, previous.revision());
            assert_eq!(*revision_after, Revision::new(1));
            assert!(*changed);
        }
        result => panic!("expected fresh session replacement, got {result:?}"),
    }
    let replacement = harness.store.current_session().unwrap();
    assert_ne!(replacement.session_id(), previous.session_id());
    assert_eq!(replacement.revision(), Revision::new(1));

    let replay = match harness.engine.admit(&replacement_request, key).unwrap() {
        AdmitOutcomeV1::Existing(JobReceiptOrTerminalV1::TerminalReceipt(receipt)) => receipt,
        outcome => panic!("expected terminal replacement replay, got {outcome:?}"),
    };
    assert_eq!(
        replay,
        PersistedTerminalReceiptV1::from_terminal_receipt(&receipt)
    );
}

#[test]
fn g006_start_replace_requires_confirmation_without_replacing_the_session() {
    let mut harness = Harness::new();
    assert_success(&harness.start());
    let before = harness.store.current_session().unwrap();
    let envelope = RequestEnvelopeV1::new(RequestEnvelopeInputV1 {
        request_id: RequestIdV1::new("00000000-0000-4000-8000-000000009711").unwrap(),
        client: ClientInfoV1::new("execution-test", "1", 1).unwrap(),
        operation: OperationV1::Mutate,
        command: CommandNameV1::new("session.start_replace").unwrap(),
        workspace: Some(
            WorkspaceContextV1::new("/worktree", Some(WorkspaceId::new(WORKSPACE_ID).unwrap()))
                .unwrap(),
        ),
        idempotency_key: Some(ProtocolIdempotencyKeyV1::new("protocol-9711").unwrap()),
        preconditions: harness.session_identity_preconditions(),
        options: RequestOptionsV1::new(false, 0).unwrap(),
        payload: json!({
            "selector": selector_json(),
            "preset": "sw-dev",
            "task_title": "Rejected replacement",
            "confirmed": false,
        })
        .as_object()
        .unwrap()
        .clone(),
    })
    .unwrap();

    assert!(SliceRequestV1::from_envelope(&envelope).is_err());
    assert_eq!(harness.store.current_session().unwrap(), before);
    assert_eq!(harness.store.request_count(), 1);
}
#[test]
fn g006_reopen_reactivates_a_completed_session_at_the_requested_stage() {
    let mut harness = Harness::new();
    assert_success(&harness.start());
    assert_success(&harness.check("confirm"));
    assert_success(&harness.attach());
    assert_success(&harness.complete());
    assert_success(&harness.check("finish"));
    assert_success(&harness.complete());

    let reopen_preconditions = harness.session_revision_preconditions();
    assert_success(&harness.submit(
        "session.reopen",
        json!({
            "selector": selector_json(),
            "destination_stage_id": "first",
            "reason": "follow up",
            "dry_run": false,
        }),
        reopen_preconditions,
    ));
    let session = harness.store.current_session().unwrap();
    assert_eq!(session.active_stage_id().unwrap().as_str(), "first");
    assert!(session.active_attempt_id().is_some());
}

#[test]
fn g006_item_uncheck_remove_clear_and_opaque_attachment_use_typed_mutations() {
    let mut harness = Harness::new();
    assert_success(&harness.start());
    assert_success(&harness.check("confirm"));
    assert_success(&harness.submit(
        "item.uncheck",
        json!({"selector": selector_json(), "item_id": "confirm"}),
        harness.item_preconditions("confirm"),
    ));
    assert_success(&harness.submit(
        "item.add",
        json!({"selector": selector_json(), "item_id": "entries", "value": "remove-me"}),
        harness.item_preconditions("entries"),
    ));
    assert_success(&harness.submit(
        "item.remove",
        json!({
            "selector": selector_json(),
            "item_id": "entries",
            "value": "remove-me",
            "ignore_missing": false,
        }),
        harness.item_preconditions("entries"),
    ));
    assert_success(&harness.submit(
        "item.set",
        json!({"selector": selector_json(), "item_id": "notes", "value": "clear-me"}),
        harness.item_preconditions("notes"),
    ));
    assert_success(&harness.submit(
        "item.clear",
        json!({"selector": selector_json(), "item_id": "notes"}),
        harness.item_preconditions("notes"),
    ));
    let attached = harness.submit(
        "item.attach",
        json!({
            "selector": selector_json(),
            "item_id": "proof",
            "reference": "build://artifact/123",
            "digest": DIGEST,
            "size_bytes": 7,
            "media_type": "text/plain",
        }),
        harness.item_preconditions("proof"),
    );
    assert_success(&attached);
    assert!(matches!(
        attached.result(),
        TerminalResultV1::Success(DomainResult::ItemChanged { .. })
    ));
}

#[test]
fn g006_stale_uncheck_is_terminal_and_does_not_partially_mutate() {
    let mut harness = Harness::new();
    assert_success(&harness.start());
    let stale_preconditions = harness.item_preconditions("confirm");
    assert_success(&harness.check("confirm"));
    let before = harness.store.current_session().unwrap();
    assert_failure(&harness.submit(
        "item.uncheck",
        json!({"selector": selector_json(), "item_id": "confirm"}),
        stale_preconditions,
    ));
    assert_eq!(harness.store.current_session().unwrap(), before);
}

#[test]
fn g006_opaque_attachment_restart_decodes_without_an_artifact_dependency() {
    #[derive(Clone, Copy)]
    struct PanicArtifacts;

    impl ArtifactVerifierV1 for PanicArtifacts {
        fn hash_local_artifact(
            &self,
            _workspace: &WorkspaceBindingV1,
            _path: &str,
            _media_type: Option<&str>,
        ) -> Result<ArtifactValueV1, ExecutionBoundaryErrorV1> {
            panic!("opaque attachment must not hash a local artifact")
        }

        fn revalidate_local_artifact(
            &self,
            _workspace: &WorkspaceBindingV1,
            _item_id: &podway_core::ItemId,
            _artifact: &ArtifactValueV1,
        ) -> Result<LocalArtifactVerificationV1, ExecutionBoundaryErrorV1> {
            panic!("opaque attachment must not revalidate a local artifact")
        }
    }

    let mut harness = Harness::new();
    assert_success(&harness.start());
    let request = slice_request(
        "item.attach",
        json!({
            "selector": selector_json(),
            "item_id": "proof",
            "reference": "build://artifact/restart",
            "digest": DIGEST,
            "size_bytes": 7,
            "media_type": "text/plain",
        }),
        harness.item_preconditions("proof"),
        970,
    );
    harness
        .engine
        .admit(&request, IdempotencyKeyV1::new("opaque-restart").unwrap())
        .unwrap();
    let restarted_clock = FixtureClock::new();
    for _ in 0..10 {
        let _ = restarted_clock.now();
    }
    let restarted = DaemonExecutionEngineV1::new(
        harness.store.clone(),
        FixtureIds::new(),
        restarted_clock,
        FixtureProcedures,
        PanicArtifacts,
        FixtureWorkspaces::stable(harness.binding.clone()),
    );
    assert_success(
        &restarted
            .execute_next(&harness.binding, WorkerIdV1::new("opaque-restart").unwrap())
            .unwrap()
            .unwrap(),
    );
}

#[test]
fn g006_workspace_reset_all_is_rejected_from_generic_admission_while_custom_procedure_start_is_admitted()
 {
    let mut harness = Harness::new();
    assert_success(&harness.submit(
        "session.start",
        json!({
            "selector": selector_json(),
            "procedure": "procedures/test.yaml",
            "task_title": "Custom procedure",
        }),
        PreconditionsV1::default(),
    ));
    let requests_before_reset = harness.store.request_count();
    let reset = slice_request(
        "workspace.reset_all",
        json!({"selector": selector_json(), "confirmed": true, "expected_workspace_uuid": WORKSPACE_ID}),
        PreconditionsV1::default(),
        9_799,
    );

    assert!(matches!(
        harness.engine.admit(
            &reset,
            IdempotencyKeyV1::new("generic-reset-rejected").unwrap(),
        ),
        Err(ExecutionErrorV1::InvalidPersistedExecution {
            reason: "workspace reset must be prepared through the maintenance path"
        })
    ));
    assert_eq!(
        harness.store.request_count(),
        requests_before_reset,
        "generic reset admission must not persist an old-generation job"
    );
}

#[test]
fn g006_destructive_commands_require_protocol_validated_confirmation() {
    let mut harness = Harness::new();
    assert_success(&harness.start());
    let envelope = RequestEnvelopeV1::new(RequestEnvelopeInputV1 {
        request_id: RequestIdV1::new("00000000-0000-4000-8000-000000009801").unwrap(),
        client: ClientInfoV1::new("execution-test", "1", 1).unwrap(),
        operation: OperationV1::Mutate,
        command: CommandNameV1::new("session.reset").unwrap(),
        workspace: Some(
            WorkspaceContextV1::new("/worktree", Some(WorkspaceId::new(WORKSPACE_ID).unwrap()))
                .unwrap(),
        ),
        idempotency_key: Some(ProtocolIdempotencyKeyV1::new("protocol-9801").unwrap()),
        preconditions: harness.session_identity_preconditions(),
        options: RequestOptionsV1::new(false, 0).unwrap(),
        payload: json!({"selector": selector_json(), "confirmed": false, "dry_run": false})
            .as_object()
            .unwrap()
            .clone(),
    })
    .unwrap();
    assert!(SliceRequestV1::from_envelope(&envelope).is_err());
    let requests_before_reset_preparation = harness.store.request_count();
    let reset_all = RequestEnvelopeV1::new(RequestEnvelopeInputV1 {
        request_id: RequestIdV1::new("00000000-0000-4000-8000-000000009802").unwrap(),
        client: ClientInfoV1::new("execution-test", "1", 1).unwrap(),
        operation: OperationV1::Bootstrap,
        command: CommandNameV1::new("workspace.reset_all").unwrap(),
        workspace: Some(
            WorkspaceContextV1::new("/worktree", Some(WorkspaceId::new(WORKSPACE_ID).unwrap()))
                .unwrap(),
        ),
        idempotency_key: Some(ProtocolIdempotencyKeyV1::new("protocol-9802").unwrap()),
        preconditions: PreconditionsV1::default(),
        options: RequestOptionsV1::new(false, 0).unwrap(),
        payload: json!({"selector": selector_json(), "confirmed": false})
            .as_object()
            .unwrap()
            .clone(),
    })
    .unwrap();
    assert!(SliceRequestV1::from_envelope(&reset_all).is_err());
    assert_eq!(
        harness.store.request_count(),
        requests_before_reset_preparation,
        "rejected confirmation must not enter Store admission"
    );
}
#[derive(Clone)]
struct ResetPreparationIds {
    calls: Arc<Mutex<Vec<&'static str>>>,
    ids: FixtureIds,
}

impl ResetPreparationIds {
    fn new() -> Self {
        Self {
            calls: Arc::new(Mutex::new(Vec::new())),
            ids: FixtureIds::new(),
        }
    }

    fn calls(&self) -> Vec<&'static str> {
        self.calls.lock().unwrap().clone()
    }

    fn record(&self, call: &'static str) {
        self.calls.lock().unwrap().push(call);
    }
}

impl ExecutionIdSourceV1 for ResetPreparationIds {
    fn next_job_id(&self) -> JobId {
        self.record("job");
        self.ids.next_job_id()
    }

    fn next_workspace_id(&self) -> WorkspaceId {
        self.record("workspace");
        WorkspaceId::new(self.ids.uuid()).unwrap()
    }

    fn next_session_id(&self) -> SessionId {
        self.ids.next_session_id()
    }

    fn next_attempt_id(&self) -> AttemptId {
        self.ids.next_attempt_id()
    }

    fn next_blocker_id(&self) -> BlockerId {
        self.ids.next_blocker_id()
    }

    fn next_procedure_snapshot_id(&self) -> ProcedureSnapshotId {
        self.ids.next_procedure_snapshot_id()
    }
}

fn reset_digest(request: &SliceRequestV1, identity: &DurableWorktreeIdentityV1) -> Sha256Digest {
    let canonical = canonical_reset_all_identity_v1(
        request,
        identity.common_dir_identity(),
        identity.worktree_admin_identity(),
    )
    .unwrap();
    Sha256Digest::new(format!("sha256:{:x}", Sha256::digest(canonical.as_bytes()))).unwrap()
}

fn seed_reset_replay(store: &RecordingStore, key: IdempotencyKeyV1, digest: Sha256Digest) -> JobId {
    let job_id = JobId::new("00000000-0000-4000-8000-000000009980").unwrap();
    let request = AdmitRequestV1::new(
        DomainCommand::WorkspaceResetAll,
        key,
        job_id.clone(),
        RevisionAttemptItemPreconditionsV1::new(None, None, None, None).unwrap(),
        digest,
        UnixMillis::new(99),
    );
    store.state.lock().unwrap().requests.push(request);
    job_id
}

#[test]
fn g006_reset_preparation_hashes_stable_identity_and_never_admits_the_old_store() {
    let previous = identity();
    let store = RecordingStore::new(previous.clone());
    let ids = ResetPreparationIds::new();
    let clock = SpyClock::new();
    let workspaces = FixtureWorkspaces::stable(binding(previous.clone()));
    let engine = DaemonExecutionEngineV1::new(
        store.clone(),
        ids.clone(),
        clock.clone(),
        FixtureProcedures,
        FixtureArtifacts,
        workspaces.clone(),
    );
    let request = slice_request(
        "workspace.reset_all",
        json!({"selector": selector_json(), "confirmed": true, "expected_workspace_uuid": WORKSPACE_ID}),
        PreconditionsV1::default(),
        9_981,
    );
    let key = IdempotencyKeyV1::new("reset-preparation-new").unwrap();

    let preparation = engine
        .prepare_workspace_reset_all(&request, &previous, key)
        .unwrap();
    let ResetAllPreparationOutcomeV1::New(preparation) = preparation else {
        panic!("new reset must produce marker inputs");
    };
    let marker = preparation.marker();

    assert_eq!(marker.request_digest(), &reset_digest(&request, &previous));
    assert_eq!(
        marker.operation_id().as_str(),
        "00000000-0000-4000-8000-000000000011"
    );
    assert_eq!(
        marker.target_workspace_uuid().as_str(),
        "00000000-0000-4000-8000-000000000012"
    );
    assert_ne!(
        marker.target_workspace_uuid(),
        previous.workspace_uuid(),
        "target workspace UUID must be fresh"
    );
    assert_eq!(marker.submitted_at_ms(), UnixMillis::new(100));
    assert_eq!(
        preparation.previous_workspace_uuid(),
        previous.workspace_uuid()
    );
    assert_eq!(store.request_count(), 0, "reset preparation must not admit");
    assert_eq!(ids.calls(), vec!["job", "workspace"]);
    assert_eq!(clock.call_count(), 1);
    assert_eq!(
        workspaces.selector_revalidations.load(Ordering::SeqCst),
        0,
        "reset preparation must not invoke Git-backed selector resolution"
    );
    assert_eq!(
        workspaces.binding_revalidations.load(Ordering::SeqCst),
        0,
        "reset preparation must not invoke Git-backed binding resolution"
    );
}
#[test]
fn g006_reset_preparation_requires_the_previous_uuid_when_store_is_readable() {
    let previous = identity();
    let store = RecordingStore::new(previous.clone());
    let ids = ResetPreparationIds::new();
    let clock = SpyClock::new();
    let workspaces = FixtureWorkspaces::stable(binding(previous.clone()));
    let engine = DaemonExecutionEngineV1::new(
        store.clone(),
        ids.clone(),
        clock.clone(),
        FixtureProcedures,
        FixtureArtifacts,
        workspaces.clone(),
    );
    let request = slice_request(
        "workspace.reset_all",
        json!({"selector": selector_json_without_expected_uuid(), "confirmed": true}),
        PreconditionsV1::default(),
        9_985,
    );

    assert!(matches!(
        engine.prepare_workspace_reset_all(
            &request,
            &previous,
            IdempotencyKeyV1::new("reset-readable-requires-uuid").unwrap(),
        ),
        Err(ExecutionErrorV1::BoundaryDomain(
            DomainError::InvalidState { .. }
        ))
    ));
    assert_eq!(
        store.request_count(),
        0,
        "missing UUID must not admit a reset job"
    );
    assert!(ids.calls().is_empty(), "missing UUID must not consume IDs");
    assert_eq!(
        clock.call_count(),
        0,
        "missing UUID must not read the clock"
    );
    assert_eq!(workspaces.selector_revalidations.load(Ordering::SeqCst), 0);
    assert_eq!(workspaces.binding_revalidations.load(Ordering::SeqCst), 0);
}

#[test]
fn g006_reset_preparation_replays_from_the_target_store_without_new_inputs() {
    let previous = identity();
    let target = DurableWorktreeIdentityV1::new(
        previous.common_dir_identity().clone(),
        WorkspaceId::new("00000000-0000-4000-8000-000000009982").unwrap(),
        previous.worktree_admin_identity().clone(),
    );
    let request = slice_request(
        "workspace.reset_all",
        json!({"selector": selector_json_with_expected(Some("00000000-0000-4000-8000-000000009982")), "confirmed": true, "expected_workspace_uuid": "00000000-0000-4000-8000-000000009982"}),
        PreconditionsV1::default(),
        9_982,
    );
    assert_eq!(
        canonical_reset_all_identity_v1(
            &request,
            previous.common_dir_identity(),
            previous.worktree_admin_identity(),
        )
        .unwrap(),
        canonical_reset_all_identity_v1(
            &request,
            target.common_dir_identity(),
            target.worktree_admin_identity(),
        )
        .unwrap(),
        "reset digest identity must survive old-to-target UUID rotation"
    );
    let key = IdempotencyKeyV1::new("reset-target-replay").unwrap();
    let store = RecordingStore::new(target.clone());
    let replay_job = seed_reset_replay(&store, key.clone(), reset_digest(&request, &previous));
    let ids = ResetPreparationIds::new();
    let clock = SpyClock::new();
    let workspaces = FixtureWorkspaces::stable(binding(target.clone()));
    let engine = DaemonExecutionEngineV1::new(
        store.clone(),
        ids.clone(),
        clock.clone(),
        FixtureProcedures,
        FixtureArtifacts,
        workspaces.clone(),
    );

    let replay = engine
        .prepare_workspace_reset_all(&request, &target, key.clone())
        .unwrap();
    match replay {
        ResetAllPreparationOutcomeV1::Existing(AdmitOutcomeV1::Existing(
            JobReceiptOrTerminalV1::JobReceipt(receipt),
        )) => assert_eq!(receipt.job_id(), &replay_job),
        outcome => panic!("expected exact Store replay, got {outcome:?}"),
    }
    let terminal = TerminalReceiptV1::new(
        JobReceiptV1::new(1, replay_job.clone(), reset_digest(&request, &previous)),
        TerminalResultV1::Success(DomainResult::WorkspaceReset {
            workspace_id: target.workspace_uuid().clone(),
            revision: Revision::ZERO,
        }),
    );
    store.state.lock().unwrap().terminal.push(terminal.clone());
    match engine
        .prepare_workspace_reset_all(&request, &target, key)
        .unwrap()
    {
        ResetAllPreparationOutcomeV1::Existing(AdmitOutcomeV1::Existing(
            JobReceiptOrTerminalV1::TerminalReceipt(receipt),
        )) => assert_eq!(
            receipt,
            PersistedTerminalReceiptV1::from_terminal_receipt(&terminal)
        ),
        outcome => panic!("expected exact terminal Store replay, got {outcome:?}"),
    }
    assert_eq!(store.request_count(), 1, "replay must not admit a reset");
    assert!(ids.calls().is_empty(), "replay must not consume IDs");
    assert_eq!(clock.call_count(), 0, "replay must not read the clock");
    assert_eq!(
        workspaces.selector_revalidations.load(Ordering::SeqCst),
        0,
        "replay must not invoke Git-backed selector resolution"
    );
    assert_eq!(
        workspaces.binding_revalidations.load(Ordering::SeqCst),
        0,
        "replay must not invoke Git-backed binding resolution"
    );
}

#[test]
fn g006_reset_preparation_rejects_store_digest_conflicts_before_generating_ids() {
    let previous = identity();
    let store = RecordingStore::new(previous.clone());
    let request = slice_request(
        "workspace.reset_all",
        json!({"selector": selector_json(), "confirmed": true, "expected_workspace_uuid": WORKSPACE_ID}),
        PreconditionsV1::default(),
        9_983,
    );
    let key = IdempotencyKeyV1::new("reset-digest-conflict").unwrap();
    seed_reset_replay(&store, key.clone(), Sha256Digest::new(DIGEST).unwrap());
    let ids = ResetPreparationIds::new();
    let clock = SpyClock::new();
    let workspaces = FixtureWorkspaces::stable(binding(previous.clone()));
    let engine = DaemonExecutionEngineV1::new(
        store.clone(),
        ids.clone(),
        clock.clone(),
        FixtureProcedures,
        FixtureArtifacts,
        workspaces.clone(),
    );

    let error = engine
        .prepare_workspace_reset_all(&request, &previous, key)
        .unwrap_err();
    match error {
        ExecutionErrorV1::Store(StoreErrorV1::IdempotencyDigestConflictV1 { expected, actual }) => {
            assert_eq!(expected, Sha256Digest::new(DIGEST).unwrap());
            assert_eq!(actual, reset_digest(&request, &previous));
        }
        error => panic!("expected exact idempotency digest conflict, got {error:?}"),
    }
    assert_eq!(store.request_count(), 1, "conflict must not admit a reset");
    assert!(ids.calls().is_empty(), "conflict must not consume IDs");
    assert_eq!(clock.call_count(), 0, "conflict must not read the clock");
    assert_eq!(workspaces.selector_revalidations.load(Ordering::SeqCst), 0);
    assert_eq!(workspaces.binding_revalidations.load(Ordering::SeqCst), 0);
}

#[test]
fn g006_reset_preparation_rejects_a_mismatched_previous_workspace_uuid() {
    let previous = identity();
    let store = RecordingStore::new(previous.clone());
    let mut mismatched_selector = selector_json();
    mismatched_selector["expected_uuid"] = json!("00000000-0000-4000-8000-000000009984");
    let request = slice_request(
        "workspace.reset_all",
        json!({
            "selector": mismatched_selector,
            "confirmed": true,
            "expected_workspace_uuid": "00000000-0000-4000-8000-000000009984",
        }),
        PreconditionsV1::default(),
        9_984,
    );
    let ids = ResetPreparationIds::new();
    let clock = SpyClock::new();
    let workspaces = FixtureWorkspaces::stable(binding(previous.clone()));
    let engine = DaemonExecutionEngineV1::new(
        store.clone(),
        ids.clone(),
        clock.clone(),
        FixtureProcedures,
        FixtureArtifacts,
        workspaces,
    );

    assert!(matches!(
        engine.prepare_workspace_reset_all(
            &request,
            &previous,
            IdempotencyKeyV1::new("reset-mismatched-previous").unwrap(),
        ),
        Err(ExecutionErrorV1::BoundaryDomain(
            DomainError::InvalidState { .. }
        ))
    ));
    assert_eq!(store.request_count(), 0, "mismatch must not admit a reset");
    assert!(ids.calls().is_empty(), "mismatch must not consume IDs");
    assert_eq!(clock.call_count(), 0, "mismatch must not read the clock");
}
#[cfg(unix)]
#[test]
fn g006_workspace_procedure_start_uses_a_bounded_regular_file_without_symlink_traversal() {
    let unique = format!(
        "podway-phase5-procedure-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let root = std::env::temp_dir().join(unique);
    std::fs::create_dir_all(&root).unwrap();
    let procedure = root.join("procedure.yaml");
    std::fs::write(&procedure, PROCEDURE_YAML).unwrap();
    let workspace = WorkspaceBindingV1::new(
        identity(),
        podway_store::ValidatedWorkspaceRootV1::from_path(&root).unwrap(),
    );

    assert!(
        EmbeddedPresetProcedureProviderV1
            .load_workspace_procedure_snapshot(
                &workspace,
                "procedure.yaml",
                ProcedureSnapshotId::new("00000000-0000-4000-8000-000000009901").unwrap(),
                UnixMillis::new(100),
            )
            .is_ok()
    );
    std::os::unix::fs::symlink(&procedure, root.join("linked.yaml")).unwrap();
    assert!(
        EmbeddedPresetProcedureProviderV1
            .load_workspace_procedure_snapshot(
                &workspace,
                "linked.yaml",
                ProcedureSnapshotId::new("00000000-0000-4000-8000-000000009902").unwrap(),
                UnixMillis::new(100),
            )
            .is_err()
    );
    std::fs::remove_dir_all(root).unwrap();
}
