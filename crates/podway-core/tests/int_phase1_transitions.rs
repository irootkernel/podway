use podway_core::{
    AddItemV1, ArtifactValueV1, AttachItemV1, AttemptId, AttemptInputV1, AttemptV1, BlockSessionV1,
    BlockerId, BlockerState, CheckItemV1, ClearItemV1, CommandContextV1, CompleteSessionV1,
    DomainCommandKind, DomainError, ItemCommonV1, ItemId, ItemMutationPreconditionsV1, ItemSpecV1,
    ItemValueV1, LocalArtifactVerificationV1, ProcedureSnapshotAssemblyInputV1,
    ProcedureSnapshotId, ProcedureSnapshotV1, ProcedureSourceLabelV1, ProcedureWarningCodeV1,
    ReopenSessionV1, ResetAllWorkspaceV1, ResetSessionV1, ReturnPolicyV1, ReturnSessionV1,
    Revision, SessionAggregateInputV1, SessionAggregateV1, SessionCommandV1, SessionId,
    SessionLifecycle, SetItemV1, Sha256Digest, SkipPolicyV1, SkipSessionV1, StageId,
    StageProgressState, StageSpecV1, StartReplaceSessionV1, StartSessionV1, TransitionEffectV1,
    UnblockSessionV1, UnixMillis, WorkspaceId, apply_transition_v1, preview_transition_v1,
};

const UUID_A: &str = "123e4567-e89b-12d3-a456-426614174000";
const UUID_B: &str = "123e4567-e89b-12d3-a456-426614174001";
const UUID_C: &str = "123e4567-e89b-12d3-a456-426614174002";
const UUID_D: &str = "123e4567-e89b-12d3-a456-426614174003";
const UUID_E: &str = "123e4567-e89b-12d3-a456-426614174004";
const UUID_F: &str = "123e4567-e89b-12d3-a456-426614174005";
const UUID_G: &str = "123e4567-e89b-12d3-a456-426614174006";
const UUID_H: &str = "123e4567-e89b-12d3-a456-426614174007";

fn digest() -> Sha256Digest {
    Sha256Digest::new(format!("sha256:{}", "a".repeat(64))).unwrap()
}

fn item_id(value: &str) -> ItemId {
    ItemId::new(value).unwrap()
}

fn stage_id(value: &str) -> StageId {
    StageId::new(value).unwrap()
}

fn common(id: &str, required: bool) -> ItemCommonV1 {
    ItemCommonV1::new(item_id(id), format!("Prompt for {id}"), None, required).unwrap()
}

fn stage(id: &str, items: Vec<ItemSpecV1>, skip_policy: SkipPolicyV1) -> StageSpecV1 {
    StageSpecV1::new(stage_id(id), id, Vec::new(), items, skip_policy).unwrap()
}

fn snapshot(
    stages: Vec<StageSpecV1>,
    accepted_warning_codes: Vec<ProcedureWarningCodeV1>,
) -> ProcedureSnapshotV1 {
    ProcedureSnapshotV1::assemble(ProcedureSnapshotAssemblyInputV1 {
        snapshot_id: ProcedureSnapshotId::new(UUID_A).unwrap(),
        procedure_id: "transition-test".to_owned(),
        procedure_version: "1".to_owned(),
        name: "Transition test".to_owned(),
        description: None,
        stages,
        return_policy: ReturnPolicyV1::any_previous(),
        source_label: ProcedureSourceLabelV1::new("test").unwrap(),
        accepted_warning_codes,
        created_at: UnixMillis::new(1),
    })
    .unwrap()
}

fn start_input(snapshot: ProcedureSnapshotV1) -> StartSessionV1 {
    StartSessionV1 {
        task_title: "Task".to_owned(),
        snapshot,
        session_id: SessionId::new(UUID_B).unwrap(),
        first_attempt_id: AttemptId::new(UUID_C).unwrap(),
    }
}

fn start(snapshot: ProcedureSnapshotV1) -> SessionAggregateV1 {
    apply_transition_v1(
        None,
        &SessionCommandV1::Start(start_input(snapshot)),
        CommandContextV1 {
            expected_revision: Revision::ZERO,
            now: UnixMillis::new(10),
        },
    )
    .unwrap()
    .next_aggregate()
    .unwrap()
    .clone()
}

fn context(session: &SessionAggregateV1, now: u64) -> CommandContextV1 {
    CommandContextV1 {
        expected_revision: session.revision(),
        now: UnixMillis::new(now),
    }
}

fn item_precondition(session: &SessionAggregateV1, item: &str) -> ItemMutationPreconditionsV1 {
    let attempt = session
        .attempts()
        .iter()
        .find(|attempt| Some(attempt.attempt_id()) == session.active_attempt_id())
        .unwrap();
    let slot = attempt
        .item_slots()
        .iter()
        .find(|slot| slot.item_id() == &item_id(item))
        .unwrap();
    ItemMutationPreconditionsV1 {
        expected_attempt_id: attempt.attempt_id().clone(),
        expected_item_revision: slot.revision(),
    }
}

fn apply(session: &SessionAggregateV1, command: SessionCommandV1, now: u64) -> SessionAggregateV1 {
    apply_transition_v1(Some(session), &command, context(session, now))
        .unwrap()
        .next_aggregate()
        .unwrap()
        .clone()
}
fn assert_complete_rejected_without_mutation(
    session: &SessionAggregateV1,
    command: CompleteSessionV1,
    now: u64,
    expected: DomainError,
) {
    let original = session.clone();
    let command = SessionCommandV1::Complete(command);
    let command_context = context(session, now);
    let preview = preview_transition_v1(Some(session), &command, command_context);
    let applied = apply_transition_v1(Some(session), &command, command_context);
    assert_eq!(preview, applied);
    assert_eq!(applied.unwrap_err(), expected);
    assert_eq!(*session, original);
    assert_eq!(session.revision(), original.revision());
}
fn assert_attempt_boundary_rejected_without_mutation(
    session: &SessionAggregateV1,
    command: SessionCommandV1,
    now: u64,
) {
    let original = session.clone();
    let command_context = context(session, now);
    let preview = preview_transition_v1(Some(session), &command, command_context);
    let applied = apply_transition_v1(Some(session), &command, command_context);
    assert_eq!(preview, applied);
    assert_eq!(
        applied.unwrap_err(),
        DomainError::InvalidState {
            reason: "attempt lifecycle timestamp must advance beyond the latest attempt boundary",
        }
    );
    assert_eq!(*session, original);
}

fn rehydrate(
    session: &SessionAggregateV1,
    attempts: Vec<AttemptV1>,
) -> Result<SessionAggregateV1, DomainError> {
    SessionAggregateV1::new(SessionAggregateInputV1 {
        session_id: session.session_id().clone(),
        task_title: session.task_title().to_owned(),
        snapshot: session.snapshot().clone(),
        lifecycle: session.lifecycle(),
        revision: session.revision(),
        stage_progress: session.stage_progress().to_vec(),
        attempts,
        active_stage_id: session.active_stage_id().cloned(),
        active_attempt_id: session.active_attempt_id().cloned(),
        created_at: session.created_at(),
        completed_at: session.completed_at(),
        cancelled_at: session.cancelled_at(),
        cancel_reason: session.cancel_reason().map(ToOwned::to_owned),
    })
}

#[test]
fn start_preview_reset_and_reset_all_are_explicit_and_deterministic() {
    let procedure = snapshot(
        vec![stage("first", Vec::new(), SkipPolicyV1::not_allowed())],
        vec![
            ProcedureWarningCodeV1::StageHasNoRequiredItems,
            ProcedureWarningCodeV1::AnyPreviousReturnPolicy,
        ],
    );
    let command = SessionCommandV1::Start(start_input(procedure.clone()));
    let start_context = CommandContextV1 {
        expected_revision: Revision::ZERO,
        now: UnixMillis::new(10),
    };
    assert_eq!(
        preview_transition_v1(None, &command, start_context),
        apply_transition_v1(None, &command, start_context)
    );
    let session = start(procedure);
    assert_eq!(session.revision(), Revision::new(1));
    assert!(apply_transition_v1(Some(&session), &command, start_context).is_err());

    let reset = apply_transition_v1(
        Some(&session),
        &SessionCommandV1::Reset(ResetSessionV1 {
            expected_session_id: session.session_id().clone(),
            confirmed: true,
        }),
        context(&session, 11),
    )
    .unwrap();
    assert!(reset.next_aggregate().is_none());

    let reset_all = apply_transition_v1(
        None,
        &SessionCommandV1::ResetAll(ResetAllWorkspaceV1 {
            workspace_id: Some(WorkspaceId::new(UUID_E).unwrap()),
            confirmed: true,
        }),
        CommandContextV1 {
            expected_revision: Revision::ZERO,
            now: UnixMillis::new(12),
        },
    )
    .unwrap();
    assert_eq!(
        reset_all.effect(),
        Some(&TransitionEffectV1::WorkspaceResetAll {
            workspace_id: Some(WorkspaceId::new(UUID_E).unwrap()),
        })
    );
}

fn assert_start_replace_success(
    prior: &SessionAggregateV1,
    command: StartReplaceSessionV1,
    now: u64,
) {
    let original = prior.clone();
    let context = context(prior, now);
    let command = SessionCommandV1::StartReplace(command);
    let preview = preview_transition_v1(Some(prior), &command, context);
    let applied = apply_transition_v1(Some(prior), &command, context);
    assert_eq!(preview, applied);

    let outcome = applied.unwrap();
    assert!(outcome.changed());
    assert_eq!(outcome.revision_before(), Some(prior.revision()));
    assert_eq!(outcome.revision_after(), Some(Revision::new(1)));

    let replacement = outcome.next_aggregate().unwrap();
    let SessionCommandV1::StartReplace(input) = &command else {
        unreachable!();
    };
    assert_eq!(replacement.session_id(), &input.start.session_id);
    assert_eq!(replacement.task_title(), input.start.task_title.as_str());
    assert_eq!(replacement.snapshot(), &input.start.snapshot);
    assert_eq!(replacement.revision(), Revision::new(1));
    assert_eq!(replacement.created_at(), UnixMillis::new(now));
    assert_eq!(replacement.latest_recorded_at(), UnixMillis::new(now));
    assert_eq!(replacement.lifecycle(), SessionLifecycle::Running);
    assert_eq!(
        replacement.active_attempt_id(),
        Some(&input.start.first_attempt_id)
    );
    assert_eq!(replacement.attempts().len(), 1);
    let attempt = &replacement.attempts()[0];
    assert_eq!(attempt.attempt_id(), &input.start.first_attempt_id);
    assert_eq!(attempt.number(), 1);
    assert_eq!(attempt.started_at(), UnixMillis::new(now));
    assert!(attempt.item_slots().iter().all(|slot| {
        slot.value().is_none()
            && slot.revision() == Revision::ZERO
            && slot.created_at().is_none()
            && slot.updated_at().is_none()
    }));
    assert_eq!(*prior, original);
}

fn assert_start_replace_rejected_without_mutation(
    prior: &SessionAggregateV1,
    command: StartReplaceSessionV1,
    now: u64,
    expected: DomainError,
) {
    let original = prior.clone();
    let command = SessionCommandV1::StartReplace(command);
    let context = context(prior, now);
    let preview = preview_transition_v1(Some(prior), &command, context);
    let applied = apply_transition_v1(Some(prior), &command, context);
    assert_eq!(preview, applied);
    assert_eq!(applied.unwrap_err(), expected);
    assert_eq!(*prior, original);
}

#[test]
fn start_replace_accepts_running_completed_and_cancelled_sessions() {
    let procedure = snapshot(
        vec![stage("first", Vec::new(), SkipPolicyV1::not_allowed())],
        vec![
            ProcedureWarningCodeV1::StageHasNoRequiredItems,
            ProcedureWarningCodeV1::AnyPreviousReturnPolicy,
        ],
    );
    let running = start(procedure.clone());

    let replacement_start = StartSessionV1 {
        task_title: "Replacement".to_owned(),
        snapshot: procedure,
        session_id: SessionId::new(UUID_D).unwrap(),
        first_attempt_id: AttemptId::new(UUID_E).unwrap(),
    };
    assert_start_replace_success(
        &running,
        StartReplaceSessionV1 {
            expected_session_id: running.session_id().clone(),
            confirmed: true,
            start: replacement_start.clone(),
        },
        11,
    );

    let completed = apply(
        &running,
        SessionCommandV1::Complete(CompleteSessionV1 {
            expected_attempt_id: running.active_attempt_id().unwrap().clone(),
            next_attempt_id: None,
            local_artifact_verifications: Vec::new(),
        }),
        11,
    );
    assert_eq!(completed.lifecycle(), SessionLifecycle::Completed);
    assert_start_replace_success(
        &completed,
        StartReplaceSessionV1 {
            expected_session_id: completed.session_id().clone(),
            confirmed: true,
            start: replacement_start.clone(),
        },
        12,
    );

    let cancelled = apply(
        &running,
        SessionCommandV1::Cancel(podway_core::CancelSessionV1 {
            expected_attempt_id: running.active_attempt_id().unwrap().clone(),
            reason: "reset required".to_owned(),
        }),
        11,
    );
    assert_eq!(cancelled.lifecycle(), SessionLifecycle::Cancelled);
    assert_start_replace_success(
        &cancelled,
        StartReplaceSessionV1 {
            expected_session_id: cancelled.session_id().clone(),
            confirmed: true,
            start: replacement_start.clone(),
        },
        12,
    );

    assert_start_replace_rejected_without_mutation(
        &running,
        StartReplaceSessionV1 {
            expected_session_id: running.session_id().clone(),
            confirmed: false,
            start: replacement_start.clone(),
        },
        11,
        DomainError::InvalidState {
            reason: "explicit confirmation is required",
        },
    );
    assert_start_replace_rejected_without_mutation(
        &running,
        StartReplaceSessionV1 {
            expected_session_id: SessionId::new(UUID_F).unwrap(),
            confirmed: true,
            start: replacement_start,
        },
        11,
        DomainError::SessionIdentityMismatch {
            expected: SessionId::new(UUID_F).unwrap(),
            actual: Some(running.session_id().clone()),
        },
    );
}

#[test]
fn item_commands_enforce_current_attempt_and_preserve_no_op_revision_stability() {
    let procedure = snapshot(
        vec![stage(
            "first",
            vec![
                ItemSpecV1::confirm(common("confirm", false)),
                ItemSpecV1::text(common("text", false), 1, 10, true).unwrap(),
                ItemSpecV1::list(common("list", false), 1, 3, 10, true).unwrap(),
            ],
            SkipPolicyV1::not_allowed(),
        )],
        vec![
            ProcedureWarningCodeV1::StageHasNoRequiredItems,
            ProcedureWarningCodeV1::AnyPreviousReturnPolicy,
        ],
    );
    let session = start(procedure);
    let stale = item_precondition(&session, "confirm");
    let session = apply(
        &session,
        SessionCommandV1::Check(CheckItemV1 {
            item_id: item_id("confirm"),
            preconditions: stale.clone(),
        }),
        10,
    );
    assert_eq!(session.revision(), Revision::new(2));
    assert!(
        apply_transition_v1(
            Some(&session),
            &SessionCommandV1::Check(CheckItemV1 {
                item_id: item_id("confirm"),
                preconditions: stale,
            }),
            context(&session, 12),
        )
        .is_err()
    );
    let session = apply(
        &session,
        SessionCommandV1::Uncheck(podway_core::UncheckItemV1 {
            item_id: item_id("confirm"),
            preconditions: item_precondition(&session, "confirm"),
        }),
        12,
    );
    let no_op = apply_transition_v1(
        Some(&session),
        &SessionCommandV1::Clear(ClearItemV1 {
            item_id: item_id("confirm"),
            preconditions: item_precondition(&session, "confirm"),
        }),
        context(&session, 13),
    )
    .unwrap();
    assert!(!no_op.changed());
    assert_eq!(no_op.revision_before(), no_op.revision_after());

    let session = apply(
        &session,
        SessionCommandV1::Set(SetItemV1 {
            item_id: item_id("text"),
            value: ItemValueV1::text("value"),
            preconditions: item_precondition(&session, "text"),
        }),
        14,
    );
    let session = apply(
        &session,
        SessionCommandV1::Add(AddItemV1 {
            item_id: item_id("list"),
            value: "first".to_owned(),
            preconditions: item_precondition(&session, "list"),
        }),
        15,
    );
    let session = apply(
        &session,
        SessionCommandV1::Add(AddItemV1 {
            item_id: item_id("list"),
            value: "second".to_owned(),
            preconditions: item_precondition(&session, "list"),
        }),
        16,
    );
    let session = apply(
        &session,
        SessionCommandV1::Remove(podway_core::RemoveItemV1 {
            item_id: item_id("list"),
            value: "first".to_owned(),
            ignore_missing: false,
            preconditions: item_precondition(&session, "list"),
        }),
        17,
    );
    let ignored = apply_transition_v1(
        Some(&session),
        &SessionCommandV1::Remove(podway_core::RemoveItemV1 {
            item_id: item_id("list"),
            value: "missing".to_owned(),
            ignore_missing: true,
            preconditions: item_precondition(&session, "list"),
        }),
        context(&session, 18),
    )
    .unwrap();
    assert!(!ignored.changed());
}

#[test]
fn text_storage_accepts_unsatisfied_values_while_completion_requires_trimmed_bounds() {
    let procedure = snapshot(
        vec![stage(
            "only",
            vec![
                ItemSpecV1::text(common("text", true), 3, 5, true).unwrap(),
                ItemSpecV1::list(common("list", true), 2, 3, 10, true).unwrap(),
            ],
            SkipPolicyV1::not_allowed(),
        )],
        vec![ProcedureWarningCodeV1::AnyPreviousReturnPolicy],
    );
    let session = start(procedure);
    let session = apply(
        &session,
        SessionCommandV1::Set(SetItemV1 {
            item_id: item_id("text"),
            value: ItemValueV1::text("ab"),
            preconditions: item_precondition(&session, "text"),
        }),
        11,
    );
    let text_slot = session.attempts()[0]
        .item_slots()
        .iter()
        .find(|slot| slot.item_id() == &item_id("text"))
        .unwrap();
    assert_eq!(text_slot.value().and_then(ItemValueV1::as_text), Some("ab"));
    assert!(
        apply_transition_v1(
            Some(&session),
            &SessionCommandV1::Complete(CompleteSessionV1 {
                expected_attempt_id: session.active_attempt_id().unwrap().clone(),
                next_attempt_id: None,
                local_artifact_verifications: Vec::new(),
            }),
            context(&session, 12),
        )
        .is_err()
    );

    let session = apply(
        &session,
        SessionCommandV1::Set(SetItemV1 {
            item_id: item_id("text"),
            value: ItemValueV1::text("abc"),
            preconditions: item_precondition(&session, "text"),
        }),
        12,
    );
    let session = apply(
        &session,
        SessionCommandV1::Add(AddItemV1 {
            item_id: item_id("list"),
            value: "first".to_owned(),
            preconditions: item_precondition(&session, "list"),
        }),
        13,
    );
    let list_slot = session.attempts()[0]
        .item_slots()
        .iter()
        .find(|slot| slot.item_id() == &item_id("list"))
        .unwrap();
    assert_eq!(
        list_slot.value().and_then(ItemValueV1::as_list),
        Some(["first".to_owned()].as_slice())
    );
    assert!(
        apply_transition_v1(
            Some(&session),
            &SessionCommandV1::Complete(CompleteSessionV1 {
                expected_attempt_id: session.active_attempt_id().unwrap().clone(),
                next_attempt_id: None,
                local_artifact_verifications: Vec::new(),
            }),
            context(&session, 14),
        )
        .is_err()
    );

    let session = apply(
        &session,
        SessionCommandV1::Remove(podway_core::RemoveItemV1 {
            item_id: item_id("list"),
            value: "first".to_owned(),
            ignore_missing: false,
            preconditions: item_precondition(&session, "list"),
        }),
        14,
    );
    let list_slot = session.attempts()[0]
        .item_slots()
        .iter()
        .find(|slot| slot.item_id() == &item_id("list"))
        .unwrap();
    assert_eq!(
        list_slot.value().and_then(ItemValueV1::as_list),
        Some([].as_slice())
    );
    assert!(
        apply_transition_v1(
            Some(&session),
            &SessionCommandV1::Complete(CompleteSessionV1 {
                expected_attempt_id: session.active_attempt_id().unwrap().clone(),
                next_attempt_id: None,
                local_artifact_verifications: Vec::new(),
            }),
            context(&session, 15),
        )
        .is_err()
    );

    let session = apply(
        &session,
        SessionCommandV1::Add(AddItemV1 {
            item_id: item_id("list"),
            value: "first".to_owned(),
            preconditions: item_precondition(&session, "list"),
        }),
        15,
    );
    let session = apply(
        &session,
        SessionCommandV1::Add(AddItemV1 {
            item_id: item_id("list"),
            value: "second".to_owned(),
            preconditions: item_precondition(&session, "list"),
        }),
        16,
    );
    let completed = apply(
        &session,
        SessionCommandV1::Complete(CompleteSessionV1 {
            expected_attempt_id: session.active_attempt_id().unwrap().clone(),
            next_attempt_id: None,
            local_artifact_verifications: Vec::new(),
        }),
        17,
    );
    assert_eq!(completed.lifecycle(), SessionLifecycle::Completed);
}

#[test]
fn complete_skip_retry_return_block_unblock_and_cancel_preserve_attempt_history() {
    let assert_skip_rejected =
        |session: &SessionAggregateV1, command: SkipSessionV1, now: u64, expected: DomainError| {
            let original = session.clone();
            let revision = session.revision();
            let command = SessionCommandV1::Skip(command);
            let command_context = context(session, now);
            let preview = preview_transition_v1(Some(session), &command, command_context);
            let applied = apply_transition_v1(Some(session), &command, command_context);
            assert_eq!(preview, applied);
            assert_eq!(applied.unwrap_err(), expected);
            assert_eq!(*session, original);
            assert_eq!(session.revision(), revision);
        };
    let procedure = snapshot(
        vec![
            stage("first", Vec::new(), SkipPolicyV1::allowed(false)),
            stage("second", Vec::new(), SkipPolicyV1::allowed(true)),
            stage("third", Vec::new(), SkipPolicyV1::not_allowed()),
        ],
        vec![
            ProcedureWarningCodeV1::StageHasNoRequiredItems,
            ProcedureWarningCodeV1::AnyPreviousReturnPolicy,
        ],
    );
    let session = start(procedure);
    let first_attempt = session.active_attempt_id().unwrap().clone();
    let session = apply(
        &session,
        SessionCommandV1::Skip(SkipSessionV1 {
            expected_attempt_id: first_attempt,
            reason: None,
            next_attempt_id: Some(AttemptId::new(UUID_D).unwrap()),
        }),
        11,
    );
    assert_eq!(
        session.stage_progress()[0].state(),
        StageProgressState::Skipped
    );
    assert_eq!(session.attempts().len(), 2);

    let second_attempt = session.active_attempt_id().unwrap().clone();
    assert_skip_rejected(
        &session,
        SkipSessionV1 {
            expected_attempt_id: second_attempt.clone(),
            reason: None,
            next_attempt_id: Some(AttemptId::new(UUID_E).unwrap()),
        },
        12,
        DomainError::InvalidState {
            reason: "a non-empty reason is required",
        },
    );
    assert_skip_rejected(
        &session,
        SkipSessionV1 {
            expected_attempt_id: second_attempt.clone(),
            reason: Some(" \t".to_owned()),
            next_attempt_id: Some(AttemptId::new(UUID_E).unwrap()),
        },
        12,
        DomainError::InvalidState {
            reason: "reason must contain at most 4000 non-blank scalars",
        },
    );
    let session = apply(
        &session,
        SessionCommandV1::Skip(SkipSessionV1 {
            expected_attempt_id: second_attempt,
            reason: Some("explicitly allowed".to_owned()),
            next_attempt_id: Some(AttemptId::new(UUID_E).unwrap()),
        }),
        12,
    );
    assert_eq!(
        session.stage_progress()[1].state(),
        StageProgressState::Skipped
    );
    assert_eq!(session.attempts().len(), 3);

    let third_attempt = session.active_attempt_id().unwrap().clone();
    assert_skip_rejected(
        &session,
        SkipSessionV1 {
            expected_attempt_id: third_attempt.clone(),
            reason: Some("policy cannot override prohibition".to_owned()),
            next_attempt_id: None,
        },
        13,
        DomainError::InvalidState {
            reason: "the active stage may not be skipped",
        },
    );
    let session = apply(
        &session,
        SessionCommandV1::Block(BlockSessionV1 {
            expected_attempt_id: third_attempt.clone(),
            blocker_id: BlockerId::new(UUID_F).unwrap(),
            reason: "waiting".to_owned(),
        }),
        13,
    );
    assert_eq!(session.attempts().len(), 3);
    assert_complete_rejected_without_mutation(
        &session,
        CompleteSessionV1 {
            expected_attempt_id: third_attempt.clone(),
            next_attempt_id: None,
            local_artifact_verifications: Vec::new(),
        },
        14,
        DomainError::BlockersPresent,
    );
    let session = apply(
        &session,
        SessionCommandV1::Unblock(UnblockSessionV1 {
            expected_attempt_id: third_attempt.clone(),
            blocker_id: None,
            unblock_all: true,
        }),
        14,
    );
    assert_eq!(session.attempts().len(), 3);
    let session = apply(
        &session,
        SessionCommandV1::Retry(podway_core::RetrySessionV1 {
            expected_attempt_id: third_attempt,
            reason: "retry".to_owned(),
            next_attempt_id: AttemptId::new(UUID_G).unwrap(),
        }),
        15,
    );
    assert_eq!(session.attempts().len(), 4);
    let session = apply(
        &session,
        SessionCommandV1::Return(ReturnSessionV1 {
            expected_attempt_id: session.active_attempt_id().unwrap().clone(),
            destination_stage_id: stage_id("first"),
            reason: "redo".to_owned(),
            destination_attempt_id: AttemptId::new(UUID_H).unwrap(),
        }),
        16,
    );
    assert_eq!(session.attempts().len(), 5);
    assert_eq!(session.active_stage_id().unwrap().as_str(), "first");
    assert_eq!(
        session.stage_progress()[2].state(),
        StageProgressState::Redo
    );
    assert_eq!(
        session
            .attempts()
            .iter()
            .filter_map(|attempt| attempt.reason())
            .collect::<Vec<_>>(),
        vec!["explicitly allowed", "retry", "redo"]
    );

    let cancelled = apply(
        &session,
        SessionCommandV1::Cancel(podway_core::CancelSessionV1 {
            expected_attempt_id: session.active_attempt_id().unwrap().clone(),
            reason: "stop".to_owned(),
        }),
        17,
    );
    assert_eq!(cancelled.lifecycle(), SessionLifecycle::Cancelled);
    assert_eq!(cancelled.attempts().len(), 5);
    assert_eq!(
        cancelled
            .attempts()
            .iter()
            .filter_map(|attempt| attempt.reason())
            .collect::<Vec<_>>(),
        vec!["stop", "explicitly allowed", "retry", "redo"]
    );
    let cancelled_original = cancelled.clone();
    let cancelled_revision = cancelled.revision();
    let command = SessionCommandV1::Retry(podway_core::RetrySessionV1 {
        expected_attempt_id: AttemptId::new(UUID_H).unwrap(),
        reason: "no".to_owned(),
        next_attempt_id: AttemptId::new("123e4567-e89b-12d3-a456-426614174008").unwrap(),
    });
    let preview = preview_transition_v1(Some(&cancelled), &command, context(&cancelled, 18));
    let applied = apply_transition_v1(Some(&cancelled), &command, context(&cancelled, 18));
    assert_eq!(preview, applied);
    assert_eq!(
        applied.unwrap_err(),
        DomainError::InvalidTransition {
            command: DomainCommandKind::SessionRetry,
            state: SessionLifecycle::Cancelled,
        }
    );
    assert_eq!(cancelled, cancelled_original);
    assert_eq!(cancelled.revision(), cancelled_revision);
}
#[test]
fn attempt_lifecycle_boundaries_advance_and_rehydrate_without_ambiguous_rework() {
    let procedure = snapshot(
        vec![
            stage("first", Vec::new(), SkipPolicyV1::not_allowed()),
            stage("second", Vec::new(), SkipPolicyV1::not_allowed()),
        ],
        vec![
            ProcedureWarningCodeV1::StageHasNoRequiredItems,
            ProcedureWarningCodeV1::AnyPreviousReturnPolicy,
        ],
    );
    let session = start(procedure);

    let session = apply(
        &session,
        SessionCommandV1::Complete(CompleteSessionV1 {
            expected_attempt_id: session.active_attempt_id().unwrap().clone(),
            next_attempt_id: Some(AttemptId::new(UUID_D).unwrap()),
            local_artifact_verifications: Vec::new(),
        }),
        11,
    );
    let second_attempt = session.active_attempt_id().unwrap().clone();
    assert_attempt_boundary_rejected_without_mutation(
        &session,
        SessionCommandV1::Retry(podway_core::RetrySessionV1 {
            expected_attempt_id: second_attempt.clone(),
            reason: "retry at the current boundary".to_owned(),
            next_attempt_id: AttemptId::new(UUID_E).unwrap(),
        }),
        11,
    );

    let session = apply(
        &session,
        SessionCommandV1::Retry(podway_core::RetrySessionV1 {
            expected_attempt_id: second_attempt,
            reason: "retry".to_owned(),
            next_attempt_id: AttemptId::new(UUID_E).unwrap(),
        }),
        12,
    );
    let retried_attempt = session.active_attempt_id().unwrap().clone();
    assert_attempt_boundary_rejected_without_mutation(
        &session,
        SessionCommandV1::Return(ReturnSessionV1 {
            expected_attempt_id: retried_attempt.clone(),
            destination_stage_id: stage_id("first"),
            reason: "return at the current boundary".to_owned(),
            destination_attempt_id: AttemptId::new(UUID_F).unwrap(),
        }),
        12,
    );

    let session = apply(
        &session,
        SessionCommandV1::Return(ReturnSessionV1 {
            expected_attempt_id: retried_attempt,
            destination_stage_id: stage_id("first"),
            reason: "return".to_owned(),
            destination_attempt_id: AttemptId::new(UUID_F).unwrap(),
        }),
        13,
    );
    assert_complete_rejected_without_mutation(
        &session,
        CompleteSessionV1 {
            expected_attempt_id: session.active_attempt_id().unwrap().clone(),
            next_attempt_id: Some(AttemptId::new(UUID_G).unwrap()),
            local_artifact_verifications: Vec::new(),
        },
        13,
        DomainError::InvalidState {
            reason: "attempt lifecycle timestamp must advance beyond the latest attempt boundary",
        },
    );

    let session = apply(
        &session,
        SessionCommandV1::Complete(CompleteSessionV1 {
            expected_attempt_id: session.active_attempt_id().unwrap().clone(),
            next_attempt_id: Some(AttemptId::new(UUID_G).unwrap()),
            local_artifact_verifications: Vec::new(),
        }),
        14,
    );
    let completed = apply(
        &session,
        SessionCommandV1::Complete(CompleteSessionV1 {
            expected_attempt_id: session.active_attempt_id().unwrap().clone(),
            next_attempt_id: None,
            local_artifact_verifications: Vec::new(),
        }),
        15,
    );
    assert_eq!(completed.lifecycle(), SessionLifecycle::Completed);
    assert_attempt_boundary_rejected_without_mutation(
        &completed,
        SessionCommandV1::Reopen(ReopenSessionV1 {
            expected_session_id: completed.session_id().clone(),
            destination_stage_id: stage_id("first"),
            reason: "reopen at the current boundary".to_owned(),
            destination_attempt_id: AttemptId::new(UUID_H).unwrap(),
        }),
        15,
    );

    let reopened = apply(
        &completed,
        SessionCommandV1::Reopen(ReopenSessionV1 {
            expected_session_id: completed.session_id().clone(),
            destination_stage_id: stage_id("first"),
            reason: "reopen".to_owned(),
            destination_attempt_id: AttemptId::new(UUID_H).unwrap(),
        }),
        16,
    );
    assert_eq!(
        rehydrate(&reopened, reopened.attempts().to_vec()).unwrap(),
        reopened
    );

    let first_stage = completed.snapshot().stages().first().unwrap();
    let mut ambiguous_attempts: Vec<_> = completed
        .attempts()
        .iter()
        .map(|attempt| {
            if attempt.stage_id() == first_stage.id() {
                AttemptV1::new(AttemptInputV1 {
                    attempt_id: attempt.attempt_id().clone(),
                    session_id: attempt.session_id().clone(),
                    stage: first_stage,
                    number: attempt.number(),
                    lifecycle: attempt.lifecycle(),
                    started_at: UnixMillis::new(10),
                    ended_at: Some(UnixMillis::new(10)),
                    reason: attempt.reason().map(ToOwned::to_owned),
                    item_slots: attempt.item_slots().to_vec(),
                    blockers: attempt.blockers().to_vec(),
                })
                .unwrap()
            } else {
                attempt.clone()
            }
        })
        .collect();
    ambiguous_attempts.reverse();
    assert_eq!(
        rehydrate(&completed, ambiguous_attempts).unwrap_err(),
        DomainError::InvalidState {
            reason: "attempt chronology is ambiguous at millisecond precision",
        }
    );
}
#[test]
fn completion_rechecks_required_local_artifact_metadata_field_by_field() {
    let artifact =
        ArtifactValueV1::local_path("reports/out.txt", digest(), 12, "text/plain").unwrap();
    let procedure = snapshot(
        vec![stage(
            "artifact",
            vec![
                ItemSpecV1::confirm(common("confirm", true)),
                ItemSpecV1::text(common("text", true), 1, 10, true).unwrap(),
                ItemSpecV1::choice(common("choice", true), vec!["yes".to_owned()]).unwrap(),
                ItemSpecV1::integer(common("integer", true), Some(1), Some(1)).unwrap(),
                ItemSpecV1::list(common("list", true), 1, 1, 10, true).unwrap(),
                ItemSpecV1::artifact(common("report", true), vec!["text/plain".to_owned()])
                    .unwrap(),
            ],
            SkipPolicyV1::not_allowed(),
        )],
        vec![ProcedureWarningCodeV1::AnyPreviousReturnPolicy],
    );
    let complete_except = |missing: &str| {
        let mut candidate = start(procedure.clone());
        if missing != "confirm" {
            candidate = apply(
                &candidate,
                SessionCommandV1::Check(CheckItemV1 {
                    item_id: item_id("confirm"),
                    preconditions: item_precondition(&candidate, "confirm"),
                }),
                11,
            );
        }
        if missing != "text" {
            candidate = apply(
                &candidate,
                SessionCommandV1::Set(SetItemV1 {
                    item_id: item_id("text"),
                    value: ItemValueV1::text("done"),
                    preconditions: item_precondition(&candidate, "text"),
                }),
                12,
            );
        }
        if missing != "choice" {
            candidate = apply(
                &candidate,
                SessionCommandV1::Set(SetItemV1 {
                    item_id: item_id("choice"),
                    value: ItemValueV1::choice("yes").unwrap(),
                    preconditions: item_precondition(&candidate, "choice"),
                }),
                13,
            );
        }
        if missing != "integer" {
            candidate = apply(
                &candidate,
                SessionCommandV1::Set(SetItemV1 {
                    item_id: item_id("integer"),
                    value: ItemValueV1::integer(1),
                    preconditions: item_precondition(&candidate, "integer"),
                }),
                14,
            );
        }
        if missing != "list" {
            candidate = apply(
                &candidate,
                SessionCommandV1::Add(AddItemV1 {
                    item_id: item_id("list"),
                    value: "entry".to_owned(),
                    preconditions: item_precondition(&candidate, "list"),
                }),
                15,
            );
        }
        candidate = apply(
            &candidate,
            SessionCommandV1::Attach(AttachItemV1 {
                item_id: item_id("report"),
                value: artifact.clone(),
                preconditions: item_precondition(&candidate, "report"),
            }),
            16,
        );
        candidate
    };
    for missing in ["confirm", "text", "choice", "integer", "list"] {
        let candidate = complete_except(missing);
        let revision = candidate.revision();
        assert_complete_rejected_without_mutation(
            &candidate,
            CompleteSessionV1 {
                expected_attempt_id: candidate.active_attempt_id().unwrap().clone(),
                next_attempt_id: None,
                local_artifact_verifications: Vec::new(),
            },
            17,
            DomainError::RequiredItemsMissing,
        );
        assert_eq!(candidate.revision(), revision);
    }

    let session = complete_except("none");
    let attempt_id = session.active_attempt_id().unwrap().clone();
    let verification = LocalArtifactVerificationV1 {
        item_id: item_id("report"),
        location: artifact.location().to_owned(),
        digest: artifact.digest().clone(),
        size_bytes: artifact.size_bytes(),
    };
    assert_complete_rejected_without_mutation(
        &session,
        CompleteSessionV1 {
            expected_attempt_id: attempt_id.clone(),
            next_attempt_id: None,
            local_artifact_verifications: Vec::new(),
        },
        17,
        DomainError::InvalidState {
            reason: "required local artifact was not verified",
        },
    );
    assert_complete_rejected_without_mutation(
        &session,
        CompleteSessionV1 {
            expected_attempt_id: attempt_id.clone(),
            next_attempt_id: None,
            local_artifact_verifications: vec![LocalArtifactVerificationV1 {
                item_id: item_id("other"),
                ..verification.clone()
            }],
        },
        17,
        DomainError::ItemNotFound {
            item_id: item_id("other"),
        },
    );
    assert_complete_rejected_without_mutation(
        &session,
        CompleteSessionV1 {
            expected_attempt_id: attempt_id.clone(),
            next_attempt_id: None,
            local_artifact_verifications: vec![LocalArtifactVerificationV1 {
                location: "reports/stale.txt".to_owned(),
                ..verification.clone()
            }],
        },
        17,
        DomainError::InvalidState {
            reason: "local artifact verification does not match the attached artifact",
        },
    );
    assert_complete_rejected_without_mutation(
        &session,
        CompleteSessionV1 {
            expected_attempt_id: attempt_id.clone(),
            next_attempt_id: None,
            local_artifact_verifications: vec![LocalArtifactVerificationV1 {
                digest: Sha256Digest::new(format!("sha256:{}", "b".repeat(64))).unwrap(),
                ..verification.clone()
            }],
        },
        17,
        DomainError::InvalidState {
            reason: "local artifact verification does not match the attached artifact",
        },
    );
    assert_complete_rejected_without_mutation(
        &session,
        CompleteSessionV1 {
            expected_attempt_id: attempt_id.clone(),
            next_attempt_id: None,
            local_artifact_verifications: vec![LocalArtifactVerificationV1 {
                size_bytes: artifact.size_bytes() + 1,
                ..verification.clone()
            }],
        },
        17,
        DomainError::InvalidState {
            reason: "local artifact verification does not match the attached artifact",
        },
    );

    let blocked = apply(
        &session,
        SessionCommandV1::Block(BlockSessionV1 {
            expected_attempt_id: attempt_id.clone(),
            blocker_id: BlockerId::new(UUID_E).unwrap(),
            reason: "verification pending".to_owned(),
        }),
        17,
    );
    assert_complete_rejected_without_mutation(
        &blocked,
        CompleteSessionV1 {
            expected_attempt_id: attempt_id,
            next_attempt_id: None,
            local_artifact_verifications: vec![verification.clone()],
        },
        18,
        DomainError::BlockersPresent,
    );
    let unblocked = apply(
        &blocked,
        SessionCommandV1::Unblock(UnblockSessionV1 {
            expected_attempt_id: blocked.active_attempt_id().unwrap().clone(),
            blocker_id: None,
            unblock_all: true,
        }),
        18,
    );
    let completed = apply(
        &unblocked,
        SessionCommandV1::Complete(CompleteSessionV1 {
            expected_attempt_id: unblocked.active_attempt_id().unwrap().clone(),
            next_attempt_id: None,
            local_artifact_verifications: vec![verification],
        }),
        19,
    );
    assert_eq!(completed.lifecycle(), SessionLifecycle::Completed);
    assert_eq!(completed.latest_recorded_at(), UnixMillis::new(19));
}

#[test]
fn reopen_requires_completed_session_and_creates_a_fresh_attempt() {
    let procedure = snapshot(
        vec![stage("only", Vec::new(), SkipPolicyV1::not_allowed())],
        vec![
            ProcedureWarningCodeV1::StageHasNoRequiredItems,
            ProcedureWarningCodeV1::AnyPreviousReturnPolicy,
        ],
    );
    let session = start(procedure);
    assert!(
        apply_transition_v1(
            Some(&session),
            &SessionCommandV1::Reopen(ReopenSessionV1 {
                expected_session_id: session.session_id().clone(),
                destination_stage_id: stage_id("only"),
                reason: "not complete yet".to_owned(),
                destination_attempt_id: AttemptId::new(UUID_D).unwrap(),
            }),
            context(&session, 11),
        )
        .is_err()
    );

    let completed = apply(
        &session,
        SessionCommandV1::Complete(CompleteSessionV1 {
            expected_attempt_id: session.active_attempt_id().unwrap().clone(),
            next_attempt_id: None,
            local_artifact_verifications: Vec::new(),
        }),
        11,
    );
    let reopened = apply(
        &completed,
        SessionCommandV1::Reopen(ReopenSessionV1 {
            expected_session_id: completed.session_id().clone(),
            destination_stage_id: stage_id("only"),
            reason: "follow-up".to_owned(),
            destination_attempt_id: AttemptId::new(UUID_D).unwrap(),
        }),
        12,
    );
    assert_eq!(reopened.lifecycle(), SessionLifecycle::Running);
    assert_eq!(reopened.attempts().len(), 2);
    let reopened_attempt_id = reopened.active_attempt_id().unwrap().clone();
    assert_eq!(reopened_attempt_id.as_str(), UUID_D);
    let reopened_attempt = reopened
        .attempts()
        .iter()
        .find(|attempt| attempt.attempt_id() == &reopened_attempt_id)
        .unwrap();
    assert_eq!(reopened_attempt.reason(), Some("follow-up"));
    let completed_again = apply(
        &reopened,
        SessionCommandV1::Complete(CompleteSessionV1 {
            expected_attempt_id: reopened_attempt_id.clone(),
            next_attempt_id: None,
            local_artifact_verifications: Vec::new(),
        }),
        13,
    );
    let completed_reopened_attempt = completed_again
        .attempts()
        .iter()
        .find(|attempt| attempt.attempt_id() == &reopened_attempt_id)
        .unwrap();
    assert_eq!(completed_reopened_attempt.reason(), Some("follow-up"));
}
#[test]
fn latest_recorded_at_is_a_causal_watermark_across_slots_and_blockers() {
    let session = start(snapshot(
        vec![stage(
            "only",
            vec![
                ItemSpecV1::text(common("left", false), 0, 10, true).unwrap(),
                ItemSpecV1::text(common("right", false), 0, 10, true).unwrap(),
            ],
            SkipPolicyV1::not_allowed(),
        )],
        vec![
            ProcedureWarningCodeV1::StageHasNoRequiredItems,
            ProcedureWarningCodeV1::AnyPreviousReturnPolicy,
        ],
    ));
    let updated_left = apply(
        &session,
        SessionCommandV1::Set(SetItemV1 {
            item_id: item_id("left"),
            value: ItemValueV1::text("left"),
            preconditions: item_precondition(&session, "left"),
        }),
        20,
    );
    assert_eq!(updated_left.latest_recorded_at(), UnixMillis::new(20));
    assert_eq!(updated_left.revision(), Revision::new(2));

    let updated_right = apply(
        &updated_left,
        SessionCommandV1::Set(SetItemV1 {
            item_id: item_id("right"),
            value: ItemValueV1::text("right"),
            preconditions: item_precondition(&updated_left, "right"),
        }),
        20,
    );
    assert_eq!(updated_right.latest_recorded_at(), UnixMillis::new(20));
    assert_eq!(updated_right.revision(), Revision::new(3));

    let before_stale_slot = updated_right.clone();
    assert_eq!(
        apply_transition_v1(
            Some(&updated_right),
            &SessionCommandV1::Set(SetItemV1 {
                item_id: item_id("right"),
                value: ItemValueV1::text("stale"),
                preconditions: item_precondition(&updated_right, "right"),
            }),
            context(&updated_right, 19),
        )
        .unwrap_err(),
        DomainError::InvalidState {
            reason: "transition timestamp precedes the latest retained timestamp",
        }
    );
    assert_eq!(updated_right, before_stale_slot);

    let blocked = apply(
        &updated_right,
        SessionCommandV1::Block(BlockSessionV1 {
            expected_attempt_id: updated_right.active_attempt_id().unwrap().clone(),
            blocker_id: BlockerId::new(UUID_E).unwrap(),
            reason: "first blocker".to_owned(),
        }),
        21,
    );
    assert_eq!(blocked.latest_recorded_at(), UnixMillis::new(21));
    assert_eq!(blocked.revision(), Revision::new(4));
    let blocker = &blocked.attempts()[0].blockers()[0];
    assert_eq!(blocker.reason(), "first blocker");
    assert_eq!(blocker.created_at(), UnixMillis::new(21));
    assert_eq!(blocker.state(), BlockerState::Open);

    let before_stale_blocker = blocked.clone();
    assert_eq!(
        apply_transition_v1(
            Some(&blocked),
            &SessionCommandV1::Block(BlockSessionV1 {
                expected_attempt_id: blocked.active_attempt_id().unwrap().clone(),
                blocker_id: BlockerId::new(UUID_F).unwrap(),
                reason: "second blocker".to_owned(),
            }),
            context(&blocked, 20),
        )
        .unwrap_err(),
        DomainError::InvalidState {
            reason: "transition timestamp precedes the latest retained timestamp",
        }
    );
    assert_eq!(blocked, before_stale_blocker);

    let unblocked = apply(
        &blocked,
        SessionCommandV1::Unblock(UnblockSessionV1 {
            expected_attempt_id: blocked.active_attempt_id().unwrap().clone(),
            blocker_id: Some(BlockerId::new(UUID_E).unwrap()),
            unblock_all: false,
        }),
        22,
    );
    assert_eq!(unblocked.latest_recorded_at(), UnixMillis::new(22));
    assert_eq!(unblocked.revision(), Revision::new(5));
    let blocker = &unblocked.attempts()[0].blockers()[0];
    assert_eq!(blocker.state(), BlockerState::Resolved);
    assert_eq!(blocker.resolved_at(), Some(UnixMillis::new(22)));

    let before_stale_resolution = unblocked.clone();
    assert_eq!(
        apply_transition_v1(
            Some(&unblocked),
            &SessionCommandV1::Block(BlockSessionV1 {
                expected_attempt_id: unblocked.active_attempt_id().unwrap().clone(),
                blocker_id: BlockerId::new(UUID_F).unwrap(),
                reason: "after resolution".to_owned(),
            }),
            context(&unblocked, 21),
        )
        .unwrap_err(),
        DomainError::InvalidState {
            reason: "transition timestamp precedes the latest retained timestamp",
        }
    );
    assert_eq!(unblocked, before_stale_resolution);
}
#[test]
fn stale_session_preconditions_fail_without_changing_the_input_aggregate() {
    let procedure = snapshot(
        vec![stage("only", Vec::new(), SkipPolicyV1::not_allowed())],
        vec![
            ProcedureWarningCodeV1::StageHasNoRequiredItems,
            ProcedureWarningCodeV1::AnyPreviousReturnPolicy,
        ],
    );
    let session = start(procedure);
    let original = session.clone();
    let error = apply_transition_v1(
        Some(&session),
        &SessionCommandV1::Cancel(podway_core::CancelSessionV1 {
            expected_attempt_id: session.active_attempt_id().unwrap().clone(),
            reason: "stop".to_owned(),
        }),
        CommandContextV1 {
            expected_revision: Revision::ZERO,
            now: UnixMillis::new(11),
        },
    )
    .unwrap_err();
    assert_eq!(
        error,
        DomainError::PreconditionFailed {
            expected: Revision::ZERO,
            actual: Revision::new(1),
        }
    );
    assert_eq!(session, original);
}

#[test]
fn rejected_terminal_ids_and_backdated_item_writes_leave_sessions_unchanged() {
    let complete_session = start(snapshot(
        vec![stage("complete", Vec::new(), SkipPolicyV1::not_allowed())],
        vec![
            ProcedureWarningCodeV1::StageHasNoRequiredItems,
            ProcedureWarningCodeV1::AnyPreviousReturnPolicy,
        ],
    ));
    let complete_original = complete_session.clone();
    assert!(
        apply_transition_v1(
            Some(&complete_session),
            &SessionCommandV1::Complete(CompleteSessionV1 {
                expected_attempt_id: complete_session.active_attempt_id().unwrap().clone(),
                next_attempt_id: Some(AttemptId::new(UUID_D).unwrap()),
                local_artifact_verifications: Vec::new(),
            }),
            context(&complete_session, 11),
        )
        .is_err()
    );
    assert_eq!(complete_session, complete_original);
    let completed = apply(
        &complete_session,
        SessionCommandV1::Complete(CompleteSessionV1 {
            expected_attempt_id: complete_session.active_attempt_id().unwrap().clone(),
            next_attempt_id: None,
            local_artifact_verifications: Vec::new(),
        }),
        11,
    );
    assert_eq!(completed.lifecycle(), SessionLifecycle::Completed);

    let skip_session = start(snapshot(
        vec![stage("skip", Vec::new(), SkipPolicyV1::allowed(false))],
        vec![
            ProcedureWarningCodeV1::StageHasNoRequiredItems,
            ProcedureWarningCodeV1::FinalStageSkippable,
            ProcedureWarningCodeV1::AnyPreviousReturnPolicy,
        ],
    ));
    let skip_original = skip_session.clone();
    assert!(
        apply_transition_v1(
            Some(&skip_session),
            &SessionCommandV1::Skip(SkipSessionV1 {
                expected_attempt_id: skip_session.active_attempt_id().unwrap().clone(),
                reason: None,
                next_attempt_id: Some(AttemptId::new(UUID_D).unwrap()),
            }),
            context(&skip_session, 11),
        )
        .is_err()
    );
    assert_eq!(skip_session, skip_original);
    let skipped = apply(
        &skip_session,
        SessionCommandV1::Skip(SkipSessionV1 {
            expected_attempt_id: skip_session.active_attempt_id().unwrap().clone(),
            reason: None,
            next_attempt_id: None,
        }),
        11,
    );
    assert_eq!(skipped.lifecycle(), SessionLifecycle::Completed);

    let non_final = start(snapshot(
        vec![
            stage("first", Vec::new(), SkipPolicyV1::allowed(false)),
            stage("second", Vec::new(), SkipPolicyV1::not_allowed()),
        ],
        vec![
            ProcedureWarningCodeV1::StageHasNoRequiredItems,
            ProcedureWarningCodeV1::AnyPreviousReturnPolicy,
        ],
    ));
    let non_final_original = non_final.clone();
    assert!(
        apply_transition_v1(
            Some(&non_final),
            &SessionCommandV1::Complete(CompleteSessionV1 {
                expected_attempt_id: non_final.active_attempt_id().unwrap().clone(),
                next_attempt_id: None,
                local_artifact_verifications: Vec::new(),
            }),
            context(&non_final, 11),
        )
        .is_err()
    );
    assert!(
        apply_transition_v1(
            Some(&non_final),
            &SessionCommandV1::Skip(SkipSessionV1 {
                expected_attempt_id: non_final.active_attempt_id().unwrap().clone(),
                reason: None,
                next_attempt_id: None,
            }),
            context(&non_final, 11),
        )
        .is_err()
    );
    assert_eq!(non_final, non_final_original);

    let text_session = start(snapshot(
        vec![stage(
            "text",
            vec![ItemSpecV1::text(common("text", false), 0, 10, true).unwrap()],
            SkipPolicyV1::not_allowed(),
        )],
        vec![
            ProcedureWarningCodeV1::StageHasNoRequiredItems,
            ProcedureWarningCodeV1::AnyPreviousReturnPolicy,
        ],
    ));
    let text_session = apply(
        &text_session,
        SessionCommandV1::Set(SetItemV1 {
            item_id: item_id("text"),
            value: ItemValueV1::text("first"),
            preconditions: item_precondition(&text_session, "text"),
        }),
        12,
    );
    let text_original = text_session.clone();
    assert!(
        apply_transition_v1(
            Some(&text_session),
            &SessionCommandV1::Set(SetItemV1 {
                item_id: item_id("text"),
                value: ItemValueV1::text("second"),
                preconditions: item_precondition(&text_session, "text"),
            }),
            context(&text_session, 11),
        )
        .is_err()
    );
    assert_eq!(text_session, text_original);
    let cleared = apply(
        &text_session,
        SessionCommandV1::Clear(ClearItemV1 {
            item_id: item_id("text"),
            preconditions: item_precondition(&text_session, "text"),
        }),
        14,
    );
    let cleared_attempt = cleared
        .attempts()
        .iter()
        .find(|attempt| Some(attempt.attempt_id()) == cleared.active_attempt_id())
        .unwrap();
    let cleared_slot = cleared_attempt
        .item_slots()
        .iter()
        .find(|slot| slot.item_id() == &item_id("text"))
        .unwrap();
    assert_eq!(cleared_slot.created_at(), Some(UnixMillis::new(12)));
    assert_eq!(cleared_slot.updated_at(), Some(UnixMillis::new(14)));
    assert!(
        apply_transition_v1(
            Some(&cleared),
            &SessionCommandV1::Set(SetItemV1 {
                item_id: item_id("text"),
                value: ItemValueV1::text("after-clear"),
                preconditions: item_precondition(&cleared, "text"),
            }),
            context(&cleared, 13),
        )
        .is_err()
    );
    assert!(
        apply_transition_v1(
            Some(&cleared),
            &SessionCommandV1::Complete(CompleteSessionV1 {
                expected_attempt_id: cleared.active_attempt_id().unwrap().clone(),
                next_attempt_id: None,
                local_artifact_verifications: Vec::new(),
            }),
            context(&cleared, 13),
        )
        .is_err()
    );
}
#[test]
fn custom_ordered_procedure_advances_in_declared_order_with_immutable_snapshot() {
    let procedure = snapshot(
        vec![
            stage("intake", Vec::new(), SkipPolicyV1::not_allowed()),
            stage("implement", Vec::new(), SkipPolicyV1::not_allowed()),
        ],
        vec![
            ProcedureWarningCodeV1::StageHasNoRequiredItems,
            ProcedureWarningCodeV1::AnyPreviousReturnPolicy,
        ],
    );
    let expected_snapshot = procedure.clone();
    let session = start(procedure);

    let advanced = apply(
        &session,
        SessionCommandV1::Complete(CompleteSessionV1 {
            expected_attempt_id: session.active_attempt_id().unwrap().clone(),
            next_attempt_id: Some(AttemptId::new(UUID_D).unwrap()),
            local_artifact_verifications: Vec::new(),
        }),
        11,
    );

    assert_eq!(advanced.active_stage_id().unwrap().as_str(), "implement");
    assert_eq!(
        advanced
            .snapshot()
            .stages()
            .iter()
            .map(|stage| stage.id().as_str())
            .collect::<Vec<_>>(),
        ["intake", "implement"]
    );
    assert_eq!(advanced.snapshot(), &expected_snapshot);
}
