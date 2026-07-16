use podway_core::{
    AddItemV1, ArtifactValueV1, AttachItemV1, AttemptId, BlockSessionV1, BlockerId,
    CancelSessionV1, CheckItemV1, CommandContextV1, CompleteSessionV1, DerivedStageStatusV1,
    ItemCommonV1, ItemId, ItemMutationPreconditionsV1, ItemSpecV1, ItemTypeV1, ItemValueV1,
    NextActionV1, ProcedureSnapshotAssemblyInputV1, ProcedureSnapshotId, ProcedureSnapshotV1,
    ProcedureSourceLabelV1, ProcedureWarningCodeV1, ReopenSessionV1, RetrySessionV1,
    ReturnPolicyV1, ReturnSessionV1, Revision, SessionAggregateV1, SessionCommandV1, SessionId,
    SessionStatusV1, SetItemV1, Sha256Digest, SkipPolicyV1, SkipSessionV1, StageId, StageSpecV1,
    UnblockSessionV1, UnixMillis, apply_transition_v1, derive_next_work_v1,
    derive_session_status_v1,
};

const UUID_A: &str = "123e4567-e89b-12d3-a456-426614174000";
const UUID_B: &str = "123e4567-e89b-12d3-a456-426614174001";
const UUID_C: &str = "123e4567-e89b-12d3-a456-426614174002";
const UUID_D: &str = "123e4567-e89b-12d3-a456-426614174003";
const UUID_E: &str = "123e4567-e89b-12d3-a456-426614174004";
const UUID_F: &str = "123e4567-e89b-12d3-a456-426614174005";
const UUID_G: &str = "123e4567-e89b-12d3-a456-426614174006";
const UUID_H: &str = "123e4567-e89b-12d3-a456-426614174007";
const UUID_I: &str = "123e4567-e89b-12d3-a456-426614174008";
const UUID_J: &str = "123e4567-e89b-12d3-a456-426614174009";

fn item_id(value: &str) -> ItemId {
    ItemId::new(value).unwrap()
}

fn stage_id(value: &str) -> StageId {
    StageId::new(value).unwrap()
}

fn item(id: &str, required: bool) -> ItemSpecV1 {
    ItemSpecV1::confirm(
        ItemCommonV1::new(item_id(id), format!("Prompt for {id}"), None, required).unwrap(),
    )
}

fn stage_with_skip_policy(
    id: &str,
    items: Vec<ItemSpecV1>,
    skip_policy: SkipPolicyV1,
) -> StageSpecV1 {
    StageSpecV1::new(
        stage_id(id),
        format!("Stage {id}"),
        vec![format!("Instruction for {id}")],
        items,
        skip_policy,
    )
    .unwrap()
}

fn stage(id: &str, items: Vec<ItemSpecV1>) -> StageSpecV1 {
    stage_with_skip_policy(id, items, SkipPolicyV1::not_allowed())
}

fn snapshot(
    stages: Vec<StageSpecV1>,
    accepted_warning_codes: Vec<ProcedureWarningCodeV1>,
) -> ProcedureSnapshotV1 {
    ProcedureSnapshotV1::assemble(ProcedureSnapshotAssemblyInputV1 {
        snapshot_id: ProcedureSnapshotId::new(UUID_A).unwrap(),
        procedure_id: "derive-test".to_owned(),
        procedure_version: "1".to_owned(),
        name: "Derive test".to_owned(),
        description: None,
        stages,
        return_policy: ReturnPolicyV1::any_previous(),
        source_label: ProcedureSourceLabelV1::new("test").unwrap(),
        accepted_warning_codes,
        created_at: UnixMillis::new(1),
    })
    .unwrap()
}
#[test]
fn snapshot_rejects_missing_warning_admission_proof() {
    assert!(
        ProcedureSnapshotV1::assemble(ProcedureSnapshotAssemblyInputV1 {
            snapshot_id: ProcedureSnapshotId::new(UUID_A).unwrap(),
            procedure_id: "derive-test".to_owned(),
            procedure_version: "1".to_owned(),
            name: "Derive test".to_owned(),
            description: None,
            stages: vec![stage("warning-stage", Vec::new())],
            return_policy: ReturnPolicyV1::any_previous(),
            source_label: ProcedureSourceLabelV1::new("test").unwrap(),
            accepted_warning_codes: Vec::new(),
            created_at: UnixMillis::new(1),
        })
        .is_err()
    );
}

fn start(snapshot: ProcedureSnapshotV1) -> SessionAggregateV1 {
    SessionAggregateV1::start(
        SessionId::new(UUID_B).unwrap(),
        "Task",
        snapshot,
        AttemptId::new(UUID_C).unwrap(),
        UnixMillis::new(10),
    )
    .unwrap()
}

fn apply(session: &SessionAggregateV1, command: SessionCommandV1, now: u64) -> SessionAggregateV1 {
    let next = apply_transition_v1(
        Some(session),
        &command,
        CommandContextV1 {
            expected_revision: session.revision(),
            now: UnixMillis::new(now),
        },
    )
    .unwrap()
    .next_aggregate()
    .unwrap()
    .clone();
    assert_eq!(next.latest_recorded_at(), UnixMillis::new(now));
    next
}

fn active_attempt(session: &SessionAggregateV1) -> &podway_core::AttemptV1 {
    session
        .attempts()
        .iter()
        .find(|attempt| Some(attempt.attempt_id()) == session.active_attempt_id())
        .unwrap()
}

fn item_preconditions(session: &SessionAggregateV1, item: &str) -> ItemMutationPreconditionsV1 {
    let attempt = active_attempt(session);
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

fn check(session: &SessionAggregateV1, item: &str, now: u64) -> SessionAggregateV1 {
    apply(
        session,
        SessionCommandV1::Check(CheckItemV1 {
            item_id: item_id(item),
            preconditions: item_preconditions(session, item),
        }),
        now,
    )
}
#[derive(Clone, Copy)]
enum ExpectedNextAction {
    Item(&'static str),
    Stage(&'static str),
    None,
}

#[allow(clippy::too_many_arguments)]
fn assert_running_projection(
    session: &SessionAggregateV1,
    expected_stages: &[(&str, DerivedStageStatusV1)],
    expected_stage_id: &str,
    expected_attempt_id: &AttemptId,
    expected_attempt_number: u32,
    expected_blocked: bool,
    expected_ready: bool,
    expected_open_blockers: &[&str],
    expected_missing_required_items: &[&str],
    expected_next_action: ExpectedNextAction,
) {
    let status = derive_session_status_v1(session);
    assert_eq!(status.status(), SessionStatusV1::Running);
    assert_eq!(
        status
            .stages()
            .iter()
            .map(|stage| (stage.stage_id().as_str(), stage.status()))
            .collect::<Vec<_>>(),
        expected_stages
    );
    assert_eq!(session.active_attempt_id(), Some(expected_attempt_id));
    let current = status.current().unwrap();
    assert_eq!(current.stage_id().as_str(), expected_stage_id);
    assert_eq!(current.attempt_id(), expected_attempt_id);
    assert_eq!(current.attempt_number(), expected_attempt_number);
    assert_eq!(current.blocked(), expected_blocked);
    assert_eq!(current.ready_to_complete(), expected_ready);
    assert_eq!(
        current
            .open_blockers()
            .iter()
            .map(|blocker| blocker.blocker_id().as_str())
            .collect::<Vec<_>>(),
        expected_open_blockers
    );

    let next = derive_next_work_v1(session);
    assert_eq!(next.status(), SessionStatusV1::Running);
    let stage = next.stage().unwrap();
    assert_eq!(stage.stage_id().as_str(), expected_stage_id);
    assert_eq!(stage.attempt_id(), expected_attempt_id);
    assert_eq!(stage.attempt_number(), expected_attempt_number);
    assert_eq!(
        next.open_blockers()
            .iter()
            .map(|blocker| blocker.blocker_id().as_str())
            .collect::<Vec<_>>(),
        expected_open_blockers
    );
    assert_eq!(
        next.missing_required_items()
            .iter()
            .map(|item| item.item_id().as_str())
            .collect::<Vec<_>>(),
        expected_missing_required_items
    );
    match (expected_next_action, next.next_action()) {
        (ExpectedNextAction::Item(expected), Some(NextActionV1::Item(item))) => {
            assert_eq!(item.item_id().as_str(), expected);
        }
        (ExpectedNextAction::Stage(expected), Some(NextActionV1::Stage(stage))) => {
            assert_eq!(stage.stage_id().as_str(), expected);
        }
        (ExpectedNextAction::None, None) => {}
        _ => panic!("derived next action does not match the transition state"),
    }
}

fn assert_terminal_projection(
    session: &SessionAggregateV1,
    expected_status: SessionStatusV1,
    expected_stages: &[(&str, DerivedStageStatusV1)],
) {
    let status = derive_session_status_v1(session);
    assert_eq!(status.status(), expected_status);
    assert_eq!(
        status
            .stages()
            .iter()
            .map(|stage| (stage.stage_id().as_str(), stage.status()))
            .collect::<Vec<_>>(),
        expected_stages
    );
    assert!(session.active_attempt_id().is_none());
    assert!(status.current().is_none());

    let next = derive_next_work_v1(session);
    assert_eq!(next.status(), expected_status);
    assert!(next.stage().is_none());
    assert!(next.open_blockers().is_empty());
    assert!(next.missing_required_items().is_empty());
    assert!(next.next_action().is_none());
    assert!(next.next_stage_after_completion().is_none());
}

#[test]
fn open_blockers_derive_blocked_while_the_stage_remains_current() {
    let session = start(snapshot(
        vec![
            stage("first", vec![item("required", true)]),
            stage("second", Vec::new()),
        ],
        vec![
            ProcedureWarningCodeV1::StageHasNoRequiredItems,
            ProcedureWarningCodeV1::AnyPreviousReturnPolicy,
        ],
    ));
    let blocked = apply(
        &session,
        SessionCommandV1::Block(BlockSessionV1 {
            expected_attempt_id: session.active_attempt_id().unwrap().clone(),
            blocker_id: BlockerId::new(UUID_D).unwrap(),
            reason: "Waiting for access".to_owned(),
        }),
        11,
    );

    let status = derive_session_status_v1(&blocked);
    let current = status.current().unwrap();
    assert_eq!(status.status(), SessionStatusV1::Running);
    assert_eq!(current.stage_id().as_str(), "first");
    assert!(current.blocked());
    assert!(!current.ready_to_complete());
    assert_eq!(current.open_blockers().len(), 1);
    assert_eq!(status.stages()[0].status(), DerivedStageStatusV1::Blocked);
    assert_eq!(status.stages()[1].status(), DerivedStageStatusV1::Pending);

    let next = derive_next_work_v1(&blocked);
    assert_eq!(next.stage().unwrap().stage_id().as_str(), "first");
    assert_eq!(next.open_blockers().len(), 1);
    assert_eq!(
        next.missing_required_items()[0].item_id().as_str(),
        "required"
    );
    assert!(next.next_action().is_none());
}
#[test]
fn open_blocker_suppresses_ready_stage_action_after_required_items_are_satisfied() {
    let session = start(snapshot(
        vec![stage("only", vec![item("required", true)])],
        vec![ProcedureWarningCodeV1::AnyPreviousReturnPolicy],
    ));
    let ready = check(&session, "required", 11);
    let blocked = apply(
        &ready,
        SessionCommandV1::Block(BlockSessionV1 {
            expected_attempt_id: ready.active_attempt_id().unwrap().clone(),
            blocker_id: BlockerId::new(UUID_E).unwrap(),
            reason: "Waiting for approval".to_owned(),
        }),
        12,
    );

    let status = derive_session_status_v1(&blocked);
    let current = status.current().unwrap();
    assert!(current.required_items()[0].satisfied());
    assert!(current.blocked());
    assert!(!current.ready_to_complete());
    assert_eq!(current.open_blockers().len(), 1);

    let next = derive_next_work_v1(&blocked);
    assert!(next.missing_required_items().is_empty());
    assert_eq!(next.open_blockers().len(), 1);
    assert_eq!(next.open_blockers()[0].blocker_id().as_str(), UUID_E);
    assert!(next.next_action().is_none());
}

#[test]
fn next_work_selects_the_first_unsatisfied_required_item_in_item_order() {
    let session = start(snapshot(
        vec![stage(
            "only",
            vec![
                item("optional-before", false),
                item("required-first", true),
                item("required-second", true),
            ],
        )],
        vec![ProcedureWarningCodeV1::AnyPreviousReturnPolicy],
    ));

    let next = derive_next_work_v1(&session);
    assert_eq!(next.missing_required_items().len(), 2);
    assert_eq!(
        next.missing_required_items()[0].item_id().as_str(),
        "required-first"
    );
    assert_eq!(
        next.missing_required_items()[1].item_id().as_str(),
        "required-second"
    );
    match next.next_action() {
        Some(NextActionV1::Item(item)) => {
            assert_eq!(item.item_id().as_str(), "required-first");
        }
        _ => panic!("first missing required item must be next"),
    }
}

#[test]
fn unsatisfied_optional_items_are_visible_but_do_not_block_stage_completion() {
    let session = start(snapshot(
        vec![stage(
            "only",
            vec![item("required", true), item("optional", false)],
        )],
        vec![ProcedureWarningCodeV1::AnyPreviousReturnPolicy],
    ));
    let session = check(&session, "required", 11);

    let status = derive_session_status_v1(&session);
    let current = status.current().unwrap();
    assert!(current.required_items()[0].satisfied());
    assert!(!current.optional_items()[0].satisfied());
    assert!(current.ready_to_complete());

    let next = derive_next_work_v1(&session);
    assert!(next.missing_required_items().is_empty());
    match next.next_action() {
        Some(NextActionV1::Stage(stage)) => assert_eq!(stage.stage_id().as_str(), "only"),
        _ => panic!("an optional item must not prevent the active stage from advancing"),
    }
}
#[test]
fn text_item_projections_preserve_metadata_satisfaction_and_revisions() {
    let session = start(snapshot(
        vec![stage(
            "collect",
            vec![
                ItemSpecV1::text(
                    ItemCommonV1::new(
                        item_id("notes"),
                        "Describe the observed result",
                        Some("Include the exact output.".to_owned()),
                        true,
                    )
                    .unwrap(),
                    3,
                    100,
                    true,
                )
                .unwrap(),
            ],
        )],
        vec![ProcedureWarningCodeV1::AnyPreviousReturnPolicy],
    ));

    let status = derive_session_status_v1(&session);
    assert_eq!(status.session_id().as_str(), UUID_B);
    assert_eq!(status.revision(), Revision::new(1));
    assert_eq!(status.status(), SessionStatusV1::Running);
    let stage = &status.stages()[0];
    assert_eq!(stage.stage_id().as_str(), "collect");
    assert_eq!(stage.stage_index(), 0);
    assert_eq!(stage.title(), "Stage collect");
    assert_eq!(stage.status(), DerivedStageStatusV1::Current);
    assert_eq!(stage.latest_attempt_number(), 1);
    let current = status
        .current()
        .expect("running session has a current stage");
    assert_eq!(current.stage_id().as_str(), "collect");
    assert_eq!(current.stage_index(), 0);
    assert_eq!(current.title(), "Stage collect");
    assert_eq!(current.attempt_id().as_str(), UUID_C);
    assert_eq!(current.attempt_number(), 1);
    assert!(!current.blocked());
    assert!(!current.ready_to_complete());
    assert!(current.open_blockers().is_empty());
    assert!(current.optional_items().is_empty());
    let progress = &current.required_items()[0];
    assert_eq!(progress.item_id().as_str(), "notes");
    assert_eq!(progress.item_type(), ItemTypeV1::Text);
    assert_eq!(progress.prompt(), "Describe the observed result");
    assert!(progress.required());
    assert!(!progress.satisfied());
    assert_eq!(progress.revision(), Revision::ZERO);

    let next = derive_next_work_v1(&session);
    assert_eq!(next.status(), SessionStatusV1::Running);
    let next_stage = next
        .stage()
        .expect("running session has next-stage metadata");
    assert_eq!(next_stage.stage_id().as_str(), "collect");
    assert_eq!(next_stage.stage_index(), 0);
    assert_eq!(next_stage.title(), "Stage collect");
    assert_eq!(next_stage.instructions().len(), 1);
    assert_eq!(next_stage.instructions()[0], "Instruction for collect");
    assert_eq!(next_stage.attempt_id().as_str(), UUID_C);
    assert_eq!(next_stage.attempt_number(), 1);
    assert!(next.open_blockers().is_empty());
    let missing = &next.missing_required_items()[0];
    assert_eq!(missing.item_id().as_str(), "notes");
    assert_eq!(missing.item_type(), ItemTypeV1::Text);
    assert_eq!(missing.prompt(), "Describe the observed result");
    assert_eq!(missing.help(), Some("Include the exact output."));
    match next.next_action() {
        Some(NextActionV1::Item(item)) => {
            assert_eq!(item.item_id().as_str(), "notes");
            assert_eq!(item.item_type(), ItemTypeV1::Text);
            assert_eq!(item.prompt(), "Describe the observed result");
            assert_eq!(item.help(), Some("Include the exact output."));
        }
        _ => panic!("missing text item must be the next action"),
    }
    assert!(next.next_stage_after_completion().is_none());

    let attempt = active_attempt(&session);
    let slot = attempt
        .item_slots()
        .iter()
        .find(|slot| slot.item_id() == &item_id("notes"))
        .expect("text item has a slot");
    let updated = apply(
        &session,
        SessionCommandV1::Set(SetItemV1 {
            item_id: item_id("notes"),
            value: ItemValueV1::text("complete observation"),
            preconditions: ItemMutationPreconditionsV1 {
                expected_attempt_id: attempt.attempt_id().clone(),
                expected_item_revision: slot.revision(),
            },
        }),
        11,
    );

    let updated_status = derive_session_status_v1(&updated);
    assert_eq!(updated_status.revision(), Revision::new(2));
    let updated_progress = &updated_status
        .current()
        .expect("updated running session has a current stage")
        .required_items()[0];
    assert_eq!(updated_progress.item_type(), ItemTypeV1::Text);
    assert!(updated_progress.satisfied());
    assert_eq!(updated_progress.revision(), Revision::new(1));
    let updated_next = derive_next_work_v1(&updated);
    assert!(updated_next.missing_required_items().is_empty());
    assert!(updated_next.next_stage_after_completion().is_none());
    match updated_next.next_action() {
        Some(NextActionV1::Stage(stage)) => assert_eq!(stage.stage_id().as_str(), "collect"),
        _ => panic!("satisfied text item must expose the stage completion action"),
    }
}
#[test]
fn ordered_all_item_kind_projections_preserve_metadata_and_progress() {
    let session = start(snapshot(
        vec![stage(
            "all-kinds",
            vec![
                ItemSpecV1::confirm(
                    ItemCommonV1::new(
                        item_id("confirm"),
                        "Confirm the result",
                        Some("Confirm help".to_owned()),
                        true,
                    )
                    .unwrap(),
                ),
                ItemSpecV1::text(
                    ItemCommonV1::new(
                        item_id("text"),
                        "Describe the result",
                        Some("Text help".to_owned()),
                        true,
                    )
                    .unwrap(),
                    1,
                    100,
                    false,
                )
                .unwrap(),
                ItemSpecV1::choice(
                    ItemCommonV1::new(
                        item_id("choice"),
                        "Choose the result",
                        Some("Choice help".to_owned()),
                        true,
                    )
                    .unwrap(),
                    vec!["alpha".to_owned(), "beta".to_owned()],
                )
                .unwrap(),
                ItemSpecV1::integer(
                    ItemCommonV1::new(
                        item_id("integer"),
                        "Count the result",
                        Some("Integer help".to_owned()),
                        true,
                    )
                    .unwrap(),
                    Some(-2),
                    Some(9),
                )
                .unwrap(),
                ItemSpecV1::list(
                    ItemCommonV1::new(
                        item_id("list"),
                        "List the results",
                        Some("List help".to_owned()),
                        true,
                    )
                    .unwrap(),
                    1,
                    2,
                    20,
                    true,
                )
                .unwrap(),
                ItemSpecV1::artifact(
                    ItemCommonV1::new(
                        item_id("artifact"),
                        "Attach the result",
                        Some("Artifact help".to_owned()),
                        true,
                    )
                    .unwrap(),
                    vec!["text/plain".to_owned()],
                )
                .unwrap(),
            ],
        )],
        vec![ProcedureWarningCodeV1::AnyPreviousReturnPolicy],
    ));
    let expected_metadata = [
        (
            "confirm",
            ItemTypeV1::Confirm,
            "Confirm the result",
            Some("Confirm help"),
        ),
        (
            "text",
            ItemTypeV1::Text,
            "Describe the result",
            Some("Text help"),
        ),
        (
            "choice",
            ItemTypeV1::Choice,
            "Choose the result",
            Some("Choice help"),
        ),
        (
            "integer",
            ItemTypeV1::Integer,
            "Count the result",
            Some("Integer help"),
        ),
        (
            "list",
            ItemTypeV1::List,
            "List the results",
            Some("List help"),
        ),
        (
            "artifact",
            ItemTypeV1::Artifact,
            "Attach the result",
            Some("Artifact help"),
        ),
    ];

    let status = derive_session_status_v1(&session);
    let current = status.current().unwrap();
    assert_eq!(
        current
            .required_items()
            .iter()
            .map(|item| {
                (
                    item.item_id().as_str(),
                    item.item_type(),
                    item.prompt(),
                    item.required(),
                )
            })
            .collect::<Vec<_>>(),
        expected_metadata
            .iter()
            .map(|(id, item_type, prompt, _)| (*id, *item_type, *prompt, true))
            .collect::<Vec<_>>(),
    );
    assert_eq!(
        current
            .required_items()
            .iter()
            .map(|item| (item.satisfied(), item.revision()))
            .collect::<Vec<_>>(),
        vec![(false, Revision::ZERO); 6],
    );

    let next = derive_next_work_v1(&session);
    assert_eq!(
        next.missing_required_items()
            .iter()
            .map(|item| {
                (
                    item.item_id().as_str(),
                    item.item_type(),
                    item.prompt(),
                    item.help(),
                )
            })
            .collect::<Vec<_>>(),
        expected_metadata.to_vec(),
    );
    match next.next_action() {
        Some(NextActionV1::Item(item)) => assert_eq!(
            (
                item.item_id().as_str(),
                item.item_type(),
                item.prompt(),
                item.help(),
            ),
            expected_metadata[0],
        ),
        _ => panic!("the first ordered item must be the next action"),
    }

    let session = check(&session, "confirm", 11);
    let session = apply(
        &session,
        SessionCommandV1::Set(SetItemV1 {
            item_id: item_id("text"),
            value: ItemValueV1::text("observed"),
            preconditions: item_preconditions(&session, "text"),
        }),
        12,
    );
    let session = apply(
        &session,
        SessionCommandV1::Set(SetItemV1 {
            item_id: item_id("choice"),
            value: ItemValueV1::choice("alpha").unwrap(),
            preconditions: item_preconditions(&session, "choice"),
        }),
        13,
    );
    let session = apply(
        &session,
        SessionCommandV1::Set(SetItemV1 {
            item_id: item_id("integer"),
            value: ItemValueV1::integer(3),
            preconditions: item_preconditions(&session, "integer"),
        }),
        14,
    );
    let session = apply(
        &session,
        SessionCommandV1::Add(AddItemV1 {
            item_id: item_id("list"),
            value: "entry".to_owned(),
            preconditions: item_preconditions(&session, "list"),
        }),
        15,
    );
    let artifact = ArtifactValueV1::external_reference(
        "artifact:all-kinds",
        Sha256Digest::new(format!("sha256:{}", "a".repeat(64))).unwrap(),
        1,
        "text/plain",
    )
    .unwrap();
    let session = apply(
        &session,
        SessionCommandV1::Attach(AttachItemV1 {
            item_id: item_id("artifact"),
            value: artifact,
            preconditions: item_preconditions(&session, "artifact"),
        }),
        16,
    );

    let status = derive_session_status_v1(&session);
    assert_eq!(status.revision(), Revision::new(7));
    let current = status.current().unwrap();
    assert!(current.ready_to_complete());
    assert_eq!(
        current
            .required_items()
            .iter()
            .map(|item| {
                (
                    item.item_id().as_str(),
                    item.item_type(),
                    item.prompt(),
                    item.required(),
                )
            })
            .collect::<Vec<_>>(),
        expected_metadata
            .iter()
            .map(|(id, item_type, prompt, _)| (*id, *item_type, *prompt, true))
            .collect::<Vec<_>>(),
    );
    assert_eq!(
        current
            .required_items()
            .iter()
            .map(|item| (item.satisfied(), item.revision()))
            .collect::<Vec<_>>(),
        vec![(true, Revision::new(1)); 6],
    );

    let next = derive_next_work_v1(&session);
    assert!(next.missing_required_items().is_empty());
    match next.next_action() {
        Some(NextActionV1::Stage(stage)) => assert_eq!(stage.stage_id().as_str(), "all-kinds"),
        _ => panic!("all satisfied ordered items must expose the stage completion action"),
    }
}
#[test]
fn completed_and_cancelled_sessions_have_no_current_or_next_action() {
    let completed_start = start(snapshot(
        vec![stage("finish", Vec::new())],
        vec![
            ProcedureWarningCodeV1::StageHasNoRequiredItems,
            ProcedureWarningCodeV1::AnyPreviousReturnPolicy,
        ],
    ));
    let completed = apply(
        &completed_start,
        SessionCommandV1::Complete(CompleteSessionV1 {
            expected_attempt_id: completed_start.active_attempt_id().unwrap().clone(),
            next_attempt_id: None,
            local_artifact_verifications: Vec::new(),
        }),
        11,
    );
    assert_terminal_projection(
        &completed,
        SessionStatusV1::Completed,
        &[("finish", DerivedStageStatusV1::Done)],
    );

    let cancelled_start = start(snapshot(
        vec![stage("interrupted", vec![item("required", true)])],
        vec![ProcedureWarningCodeV1::AnyPreviousReturnPolicy],
    ));
    let blocked = apply(
        &cancelled_start,
        SessionCommandV1::Block(BlockSessionV1 {
            expected_attempt_id: cancelled_start.active_attempt_id().unwrap().clone(),
            blocker_id: BlockerId::new(UUID_D).unwrap(),
            reason: "Waiting for cancellation".to_owned(),
        }),
        11,
    );
    assert_running_projection(
        &blocked,
        &[("interrupted", DerivedStageStatusV1::Blocked)],
        "interrupted",
        &AttemptId::new(UUID_C).unwrap(),
        1,
        true,
        false,
        &[UUID_D],
        &["required"],
        ExpectedNextAction::None,
    );
    assert!(
        !derive_session_status_v1(&blocked)
            .current()
            .unwrap()
            .required_items()[0]
            .satisfied()
    );
    let cancelled = apply(
        &blocked,
        SessionCommandV1::Cancel(CancelSessionV1 {
            expected_attempt_id: blocked.active_attempt_id().unwrap().clone(),
            reason: "Stop work".to_owned(),
        }),
        12,
    );
    assert_terminal_projection(
        &cancelled,
        SessionStatusV1::Cancelled,
        &[("interrupted", DerivedStageStatusV1::Abandoned)],
    );
}

#[test]
fn derived_stage_and_item_order_follow_the_immutable_procedure_order() {
    let session = start(snapshot(
        vec![
            stage(
                "zeta",
                vec![item("required-second", true), item("required-first", true)],
            ),
            stage("alpha", Vec::new()),
        ],
        vec![
            ProcedureWarningCodeV1::StageHasNoRequiredItems,
            ProcedureWarningCodeV1::AnyPreviousReturnPolicy,
        ],
    ));

    let status = derive_session_status_v1(&session);
    assert_eq!(status.stages()[0].stage_id().as_str(), "zeta");
    assert_eq!(status.stages()[0].stage_index(), 0);
    assert_eq!(status.stages()[1].stage_id().as_str(), "alpha");
    assert_eq!(status.stages()[1].stage_index(), 1);

    let next = derive_next_work_v1(&session);
    assert_eq!(
        next.missing_required_items()[0].item_id().as_str(),
        "required-second"
    );
    assert_eq!(
        next.missing_required_items()[1].item_id().as_str(),
        "required-first"
    );
    assert_eq!(
        next.next_stage_after_completion()
            .unwrap()
            .stage_id()
            .as_str(),
        "alpha"
    );
    assert_eq!(next.next_stage_after_completion().unwrap().stage_index(), 1);
}
#[test]
fn projections_follow_complete_skip_retry_return_reopen_block_unblock_and_cancel() {
    let first_attempt_id = AttemptId::new(UUID_C).unwrap();
    let skipped_attempt_id = AttemptId::new(UUID_D).unwrap();
    let retried_attempt_id = AttemptId::new(UUID_E).unwrap();
    let third_attempt_id = AttemptId::new(UUID_F).unwrap();
    let returned_attempt_id = AttemptId::new(UUID_G).unwrap();
    let final_attempt_id = AttemptId::new(UUID_H).unwrap();
    let reopened_attempt_id = AttemptId::new(UUID_I).unwrap();
    let blocker_id = BlockerId::new(UUID_J).unwrap();

    let session = start(snapshot(
        vec![
            stage_with_skip_policy("first", Vec::new(), SkipPolicyV1::allowed(true)),
            stage("second", vec![item("required", true)]),
            stage("third", Vec::new()),
        ],
        vec![
            ProcedureWarningCodeV1::StageHasNoRequiredItems,
            ProcedureWarningCodeV1::AnyPreviousReturnPolicy,
        ],
    ));
    let initial_stages = [
        ("first", DerivedStageStatusV1::Current),
        ("second", DerivedStageStatusV1::Pending),
        ("third", DerivedStageStatusV1::Pending),
    ];
    assert_running_projection(
        &session,
        &initial_stages,
        "first",
        &first_attempt_id,
        1,
        false,
        true,
        &[],
        &[],
        ExpectedNextAction::Stage("first"),
    );

    let skipped = apply(
        &session,
        SessionCommandV1::Skip(SkipSessionV1 {
            expected_attempt_id: first_attempt_id.clone(),
            reason: Some("not needed".to_owned()),
            next_attempt_id: Some(skipped_attempt_id.clone()),
        }),
        11,
    );
    let skipped_stages = [
        ("first", DerivedStageStatusV1::Skipped),
        ("second", DerivedStageStatusV1::Current),
        ("third", DerivedStageStatusV1::Pending),
    ];
    assert_running_projection(
        &skipped,
        &skipped_stages,
        "second",
        &skipped_attempt_id,
        1,
        false,
        false,
        &[],
        &["required"],
        ExpectedNextAction::Item("required"),
    );

    let retried = apply(
        &skipped,
        SessionCommandV1::Retry(RetrySessionV1 {
            expected_attempt_id: skipped_attempt_id.clone(),
            reason: "retry second".to_owned(),
            next_attempt_id: retried_attempt_id.clone(),
        }),
        12,
    );
    assert_running_projection(
        &retried,
        &skipped_stages,
        "second",
        &retried_attempt_id,
        2,
        false,
        false,
        &[],
        &["required"],
        ExpectedNextAction::Item("required"),
    );

    let completed_second = check(&retried, "required", 13);
    assert_running_projection(
        &completed_second,
        &skipped_stages,
        "second",
        &retried_attempt_id,
        2,
        false,
        true,
        &[],
        &[],
        ExpectedNextAction::Stage("second"),
    );
    let on_third = apply(
        &completed_second,
        SessionCommandV1::Complete(CompleteSessionV1 {
            expected_attempt_id: retried_attempt_id.clone(),
            next_attempt_id: Some(third_attempt_id.clone()),
            local_artifact_verifications: Vec::new(),
        }),
        14,
    );
    let third_stages = [
        ("first", DerivedStageStatusV1::Skipped),
        ("second", DerivedStageStatusV1::Done),
        ("third", DerivedStageStatusV1::Current),
    ];
    assert_running_projection(
        &on_third,
        &third_stages,
        "third",
        &third_attempt_id,
        1,
        false,
        true,
        &[],
        &[],
        ExpectedNextAction::Stage("third"),
    );

    let returned = apply(
        &on_third,
        SessionCommandV1::Return(ReturnSessionV1 {
            expected_attempt_id: third_attempt_id.clone(),
            destination_stage_id: stage_id("second"),
            reason: "redo second".to_owned(),
            destination_attempt_id: returned_attempt_id.clone(),
        }),
        15,
    );
    let returned_stages = [
        ("first", DerivedStageStatusV1::Skipped),
        ("second", DerivedStageStatusV1::Current),
        ("third", DerivedStageStatusV1::Redo),
    ];
    assert_running_projection(
        &returned,
        &returned_stages,
        "second",
        &returned_attempt_id,
        3,
        false,
        false,
        &[],
        &["required"],
        ExpectedNextAction::Item("required"),
    );

    let returned_checked = check(&returned, "required", 16);
    assert_running_projection(
        &returned_checked,
        &returned_stages,
        "second",
        &returned_attempt_id,
        3,
        false,
        true,
        &[],
        &[],
        ExpectedNextAction::Stage("second"),
    );
    let final_stage = apply(
        &returned_checked,
        SessionCommandV1::Complete(CompleteSessionV1 {
            expected_attempt_id: returned_attempt_id.clone(),
            next_attempt_id: Some(final_attempt_id.clone()),
            local_artifact_verifications: Vec::new(),
        }),
        17,
    );
    assert_running_projection(
        &final_stage,
        &third_stages,
        "third",
        &final_attempt_id,
        2,
        false,
        true,
        &[],
        &[],
        ExpectedNextAction::Stage("third"),
    );

    let completed = apply(
        &final_stage,
        SessionCommandV1::Complete(CompleteSessionV1 {
            expected_attempt_id: final_attempt_id.clone(),
            next_attempt_id: None,
            local_artifact_verifications: Vec::new(),
        }),
        18,
    );
    let completed_stages = [
        ("first", DerivedStageStatusV1::Skipped),
        ("second", DerivedStageStatusV1::Done),
        ("third", DerivedStageStatusV1::Done),
    ];
    assert_terminal_projection(&completed, SessionStatusV1::Completed, &completed_stages);

    let reopened = apply(
        &completed,
        SessionCommandV1::Reopen(ReopenSessionV1 {
            expected_session_id: completed.session_id().clone(),
            destination_stage_id: stage_id("second"),
            reason: "follow-up".to_owned(),
            destination_attempt_id: reopened_attempt_id.clone(),
        }),
        19,
    );
    assert_running_projection(
        &reopened,
        &returned_stages,
        "second",
        &reopened_attempt_id,
        4,
        false,
        false,
        &[],
        &["required"],
        ExpectedNextAction::Item("required"),
    );
    assert_eq!(active_attempt(&reopened).reason(), Some("follow-up"));

    let blocked = apply(
        &reopened,
        SessionCommandV1::Block(BlockSessionV1 {
            expected_attempt_id: reopened_attempt_id.clone(),
            blocker_id: blocker_id.clone(),
            reason: "waiting".to_owned(),
        }),
        20,
    );
    assert_running_projection(
        &blocked,
        &[
            ("first", DerivedStageStatusV1::Skipped),
            ("second", DerivedStageStatusV1::Blocked),
            ("third", DerivedStageStatusV1::Redo),
        ],
        "second",
        &reopened_attempt_id,
        4,
        true,
        false,
        &[UUID_J],
        &["required"],
        ExpectedNextAction::None,
    );

    let unblocked = apply(
        &blocked,
        SessionCommandV1::Unblock(UnblockSessionV1 {
            expected_attempt_id: reopened_attempt_id.clone(),
            blocker_id: Some(blocker_id),
            unblock_all: false,
        }),
        21,
    );
    assert_running_projection(
        &unblocked,
        &returned_stages,
        "second",
        &reopened_attempt_id,
        4,
        false,
        false,
        &[],
        &["required"],
        ExpectedNextAction::Item("required"),
    );

    let reopened_checked = check(&unblocked, "required", 22);
    assert_running_projection(
        &reopened_checked,
        &returned_stages,
        "second",
        &reopened_attempt_id,
        4,
        false,
        true,
        &[],
        &[],
        ExpectedNextAction::Stage("second"),
    );

    let cancelled = apply(
        &reopened_checked,
        SessionCommandV1::Cancel(CancelSessionV1 {
            expected_attempt_id: reopened_attempt_id,
            reason: "cancel after reopen".to_owned(),
        }),
        23,
    );
    assert_terminal_projection(
        &cancelled,
        SessionStatusV1::Cancelled,
        &[
            ("first", DerivedStageStatusV1::Skipped),
            ("second", DerivedStageStatusV1::Abandoned),
            ("third", DerivedStageStatusV1::Redo),
        ],
    );
    let cancelled_reopened_attempt = cancelled
        .attempts()
        .iter()
        .find(|attempt| attempt.stage_id().as_str() == "second" && attempt.number() == 4)
        .unwrap();
    assert_eq!(
        cancelled_reopened_attempt.reason(),
        Some("cancel after reopen")
    );
}
