use podway_core::{
    ArtifactValueV1, AttemptId, AttemptInputV1, AttemptLifecycle, AttemptV1, BlockSessionV1,
    BlockerId, BlockerInputV1, BlockerState, BlockerV1, CommandContextV1, DomainError,
    ItemCommonV1, ItemId, ItemSlotInputV1, ItemSlotV1, ItemSpecV1, ItemValueV1,
    MAX_OPEN_BLOCKERS_PER_ATTEMPT_V1, MAX_TEXT_LENGTH, ProcedureSnapshotAssemblyInputV1,
    ProcedureSnapshotId, ProcedureSnapshotV1, ProcedureSourceLabelV1, ProcedureWarningCodeV1,
    RetrySessionV1, ReturnPolicyV1, Revision, SessionAggregateInputV1, SessionAggregateV1,
    SessionCommandV1, SessionId, SessionLifecycle, Sha256Digest, SkipPolicyV1, StageId,
    StageProgressState, StageProgressV1, StageSpecV1, UnblockSessionV1, UnixMillis,
    apply_transition_v1, item_satisfied, required_items_satisfied,
};

const UUID_A: &str = "123e4567-e89b-12d3-a456-426614174000";
const UUID_B: &str = "123e4567-e89b-12d3-a456-426614174001";
const UUID_C: &str = "123e4567-e89b-12d3-a456-426614174002";
const UUID_D: &str = "123e4567-e89b-12d3-a456-426614174003";

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

fn procedure_snapshot(
    stages: Vec<StageSpecV1>,
    accepted_warning_codes: Vec<ProcedureWarningCodeV1>,
) -> ProcedureSnapshotV1 {
    ProcedureSnapshotV1::assemble(ProcedureSnapshotAssemblyInputV1 {
        snapshot_id: ProcedureSnapshotId::new(UUID_A).unwrap(),
        procedure_id: "phase-one-test".to_owned(),
        procedure_version: "1".to_owned(),
        name: "Phase one test".to_owned(),
        description: None,
        stages,
        return_policy: ReturnPolicyV1::any_previous(),
        source_label: ProcedureSourceLabelV1::new("preset: phase-one-test").unwrap(),
        accepted_warning_codes,
        created_at: UnixMillis::new(10),
    })
    .unwrap()
}
struct PlainAttemptFixture<'a> {
    attempt: &'a str,
    session_id: &'a SessionId,
    stage: &'a StageSpecV1,
    number: u32,
    lifecycle: AttemptLifecycle,
    started_at: u64,
    ended_at: Option<u64>,
    reason: Option<&'a str>,
}

fn plain_attempt(fixture: PlainAttemptFixture<'_>) -> AttemptV1 {
    AttemptV1::new(AttemptInputV1 {
        attempt_id: AttemptId::new(fixture.attempt).unwrap(),
        session_id: fixture.session_id.clone(),
        stage: fixture.stage,
        number: fixture.number,
        lifecycle: fixture.lifecycle,
        started_at: UnixMillis::new(fixture.started_at),
        ended_at: fixture.ended_at.map(UnixMillis::new),
        reason: fixture.reason.map(ToOwned::to_owned),
        item_slots: fixture
            .stage
            .items()
            .iter()
            .map(|item| ItemSlotV1::new_empty(AttemptId::new(fixture.attempt).unwrap(), item))
            .collect(),
        blockers: Vec::new(),
    })
    .unwrap()
}
#[allow(clippy::too_many_arguments)]
fn hydrated_attempt(
    attempt: &str,
    session_id: &SessionId,
    stage: &StageSpecV1,
    number: u32,
    lifecycle: AttemptLifecycle,
    started_at: u64,
    ended_at: Option<u64>,
    reason: Option<&str>,
    blockers: Vec<BlockerV1>,
) -> Result<AttemptV1, DomainError> {
    let attempt_id = AttemptId::new(attempt).unwrap();
    AttemptV1::new(AttemptInputV1 {
        attempt_id: attempt_id.clone(),
        session_id: session_id.clone(),
        stage,
        number,
        lifecycle,
        started_at: UnixMillis::new(started_at),
        ended_at: ended_at.map(UnixMillis::new),
        reason: reason.map(ToOwned::to_owned),
        item_slots: stage
            .items()
            .iter()
            .map(|item| ItemSlotV1::new_empty(attempt_id.clone(), item))
            .collect(),
        blockers,
    })
}
fn hydrated_active_attempt(
    attempt_id: &AttemptId,
    session_id: &SessionId,
    stage: &StageSpecV1,
    item_slots: Vec<ItemSlotV1>,
) -> AttemptV1 {
    AttemptV1::new(AttemptInputV1 {
        attempt_id: attempt_id.clone(),
        session_id: session_id.clone(),
        stage,
        number: 1,
        lifecycle: AttemptLifecycle::Active,
        started_at: UnixMillis::new(10),
        ended_at: None,
        reason: None,
        item_slots,
        blockers: Vec::new(),
    })
    .unwrap()
}

#[allow(clippy::too_many_arguments)]
fn hydrated_aggregate(
    session_id: SessionId,
    snapshot: ProcedureSnapshotV1,
    lifecycle: SessionLifecycle,
    revision: u64,
    stage_progress: Vec<StageProgressV1>,
    attempts: Vec<AttemptV1>,
    active_stage_id: Option<StageId>,
    active_attempt_id: Option<AttemptId>,
    created_at: u64,
    completed_at: Option<u64>,
    cancelled_at: Option<u64>,
    cancel_reason: Option<&str>,
) -> Result<SessionAggregateV1, DomainError> {
    SessionAggregateV1::new(SessionAggregateInputV1 {
        session_id,
        task_title: "Hydrated history".to_owned(),
        snapshot,
        lifecycle,
        revision: Revision::new(revision),
        stage_progress,
        attempts,
        active_stage_id,
        active_attempt_id,
        created_at: UnixMillis::new(created_at),
        completed_at: completed_at.map(UnixMillis::new),
        cancelled_at: cancelled_at.map(UnixMillis::new),
        cancel_reason: cancel_reason.map(ToOwned::to_owned),
    })
}

fn attempt_by_key<'a>(
    aggregate: &'a SessionAggregateV1,
    stage_id: &StageId,
    attempt_id: &AttemptId,
    number: u32,
) -> &'a AttemptV1 {
    aggregate
        .attempts()
        .iter()
        .find(|attempt| {
            attempt.stage_id() == stage_id
                && attempt.attempt_id() == attempt_id
                && attempt.number() == number
        })
        .unwrap()
}
fn assert_invalid_state<T>(result: Result<T, DomainError>, reason: &'static str) {
    match result {
        Ok(_) => panic!("expected invalid state: {reason}"),
        Err(error) => assert_eq!(error, DomainError::InvalidState { reason }),
    }
}
#[test]
fn hydration_rejects_foreign_relationships_and_invalid_item_slot_sets() {
    let first = ItemSpecV1::confirm(common("first", false));
    let second = ItemSpecV1::confirm(common("second", false));
    let foreign = ItemSpecV1::confirm(common("foreign", false));
    let stage = StageSpecV1::new(
        stage_id("hydration-relations"),
        "Hydration relations",
        Vec::new(),
        vec![first.clone(), second.clone()],
        SkipPolicyV1::not_allowed(),
    )
    .unwrap();
    let snapshot = procedure_snapshot(
        vec![stage.clone()],
        vec![
            ProcedureWarningCodeV1::StageHasNoRequiredItems,
            ProcedureWarningCodeV1::AnyPreviousReturnPolicy,
        ],
    );
    let session_id = SessionId::new(UUID_B).unwrap();
    let attempt_id = AttemptId::new(UUID_C).unwrap();
    let foreign_attempt_id = AttemptId::new(UUID_D).unwrap();
    let first_slot = ItemSlotV1::new_empty(attempt_id.clone(), &first);
    let second_slot = ItemSlotV1::new_empty(attempt_id.clone(), &second);

    assert_invalid_state(
        AttemptV1::new(AttemptInputV1 {
            attempt_id: attempt_id.clone(),
            session_id: session_id.clone(),
            stage: &stage,
            number: 1,
            lifecycle: AttemptLifecycle::Active,
            started_at: UnixMillis::new(10),
            ended_at: None,
            reason: None,
            item_slots: vec![first_slot.clone()],
            blockers: Vec::new(),
        }),
        "attempt must have exactly one slot for every stage item",
    );
    assert_invalid_state(
        AttemptV1::new(AttemptInputV1 {
            attempt_id: attempt_id.clone(),
            session_id: session_id.clone(),
            stage: &stage,
            number: 1,
            lifecycle: AttemptLifecycle::Active,
            started_at: UnixMillis::new(10),
            ended_at: None,
            reason: None,
            item_slots: vec![first_slot.clone(), first_slot.clone()],
            blockers: Vec::new(),
        }),
        "item slot does not match its specification",
    );
    assert_invalid_state(
        AttemptV1::new(AttemptInputV1 {
            attempt_id: attempt_id.clone(),
            session_id: session_id.clone(),
            stage: &stage,
            number: 1,
            lifecycle: AttemptLifecycle::Active,
            started_at: UnixMillis::new(10),
            ended_at: None,
            reason: None,
            item_slots: vec![
                ItemSlotV1::new_empty(attempt_id.clone(), &foreign),
                second_slot.clone(),
            ],
            blockers: Vec::new(),
        }),
        "item slot does not match its specification",
    );
    assert_invalid_state(
        AttemptV1::new(AttemptInputV1 {
            attempt_id: attempt_id.clone(),
            session_id: session_id.clone(),
            stage: &stage,
            number: 1,
            lifecycle: AttemptLifecycle::Active,
            started_at: UnixMillis::new(10),
            ended_at: None,
            reason: None,
            item_slots: vec![
                ItemSlotV1::new_empty(foreign_attempt_id.clone(), &first),
                second_slot.clone(),
            ],
            blockers: Vec::new(),
        }),
        "item slot belongs to another attempt",
    );

    let foreign_blocker = BlockerV1::open(
        BlockerId::new(UUID_A).unwrap(),
        foreign_attempt_id.clone(),
        "foreign attempt",
        UnixMillis::new(10),
    )
    .unwrap();
    assert_invalid_state(
        AttemptV1::new(AttemptInputV1 {
            attempt_id: attempt_id.clone(),
            session_id: session_id.clone(),
            stage: &stage,
            number: 1,
            lifecycle: AttemptLifecycle::Active,
            started_at: UnixMillis::new(10),
            ended_at: None,
            reason: None,
            item_slots: vec![first_slot, second_slot],
            blockers: vec![foreign_blocker],
        }),
        "blocker belongs to another attempt",
    );

    let foreign_session = SessionId::new(UUID_A).unwrap();
    let foreign_attempt = AttemptV1::new(AttemptInputV1 {
        attempt_id: foreign_attempt_id.clone(),
        session_id: foreign_session,
        stage: &stage,
        number: 1,
        lifecycle: AttemptLifecycle::Active,
        started_at: UnixMillis::new(10),
        ended_at: None,
        reason: None,
        item_slots: vec![
            ItemSlotV1::new_empty(foreign_attempt_id.clone(), &first),
            ItemSlotV1::new_empty(foreign_attempt_id.clone(), &second),
        ],
        blockers: Vec::new(),
    })
    .unwrap();
    assert_invalid_state(
        hydrated_aggregate(
            session_id,
            snapshot,
            SessionLifecycle::Running,
            1,
            vec![
                StageProgressV1::current(stage.id().clone(), 0, foreign_attempt_id.clone(), 1)
                    .unwrap(),
            ],
            vec![foreign_attempt],
            Some(stage.id().clone()),
            Some(foreign_attempt_id),
            10,
            None,
            None,
            None,
        ),
        "attempt belongs to another session",
    );
}

#[test]
fn every_item_type_uses_exact_value_validation_and_preserves_text_and_list_order() {
    let confirm = ItemSpecV1::confirm(common("confirmed", true));
    let text = ItemSpecV1::text(common("text", true), 1, 1, true).unwrap();
    let choice = ItemSpecV1::choice(
        common("choice", true),
        vec!["low".to_owned(), "high".to_owned()],
    )
    .unwrap();
    let integer = ItemSpecV1::integer(common("integer", true), Some(-2), Some(2)).unwrap();
    let list = ItemSpecV1::list(common("list", true), 1, 2, 8, true).unwrap();
    let artifact =
        ItemSpecV1::artifact(common("artifact", true), vec!["text/plain".to_owned()]).unwrap();
    assert!(ItemSpecV1::text(common("invalid-text", true), 2, 1, true).is_err());
    assert!(ItemSpecV1::list(common("invalid-list", true), 2, 1, 8, true).is_err());
    assert!(
        ItemSpecV1::choice(
            common("duplicate-choice", true),
            vec!["same".to_owned(), "same".to_owned()],
        )
        .is_err()
    );

    let text_value = ItemValueV1::text("\u{2003}é\u{2002}");
    let list_value = ItemValueV1::list(vec!["first".to_owned(), "second".to_owned()]).unwrap();
    let artifact_value =
        ArtifactValueV1::local_path("tests/example.txt", digest(), 7, "text/plain").unwrap();

    assert!(item_satisfied(&confirm, Some(&ItemValueV1::confirm())));
    assert!(item_satisfied(&text, Some(&text_value)));
    assert_eq!(text_value.as_text(), Some("\u{2003}é\u{2002}"));
    assert!(item_satisfied(
        &choice,
        Some(&ItemValueV1::choice("low").unwrap())
    ));
    assert!(item_satisfied(&integer, Some(&ItemValueV1::integer(-2))));
    assert!(item_satisfied(&list, Some(&list_value)));
    assert_eq!(
        list_value.as_list(),
        Some(["first".to_owned(), "second".to_owned()].as_slice())
    );
    assert!(item_satisfied(
        &artifact,
        Some(&ItemValueV1::artifact(artifact_value))
    ));

    assert!(!item_satisfied(&text, Some(&ItemValueV1::text("\u{2003}"))));
    assert!(!item_satisfied(
        &choice,
        Some(&ItemValueV1::choice("LOW").unwrap())
    ));
    assert!(!item_satisfied(&integer, Some(&ItemValueV1::integer(3))));
    assert!(!item_satisfied(
        &list,
        Some(&ItemValueV1::list(vec!["first".to_owned(), "first".to_owned()]).unwrap())
    ));
    assert!(!item_satisfied(&text, Some(&ItemValueV1::integer(1))));
}

#[test]
fn whole_list_writes_are_one_slot_transition_regardless_of_cardinality() {
    let attempt_id = AttemptId::new(UUID_B).unwrap();
    let list = ItemSpecV1::list(common("list", false), 0, 3, 8, true).unwrap();

    let first_empty_write = ItemSlotV1::new_empty(attempt_id.clone(), &list)
        .with_value(
            &list,
            ItemValueV1::list(Vec::new()).unwrap(),
            UnixMillis::new(100),
        )
        .unwrap();
    assert_eq!(first_empty_write.revision(), Revision::new(1));

    let first_multi_write = ItemSlotV1::new_empty(attempt_id.clone(), &list)
        .with_value(
            &list,
            ItemValueV1::list(vec!["one".to_owned(), "two".to_owned()]).unwrap(),
            UnixMillis::new(100),
        )
        .unwrap();
    assert_eq!(first_multi_write.revision(), Revision::new(1));

    let same_cardinality_replacement = first_multi_write
        .with_value(
            &list,
            ItemValueV1::list(vec!["three".to_owned(), "four".to_owned()]).unwrap(),
            UnixMillis::new(101),
        )
        .unwrap();
    assert_eq!(same_cardinality_replacement.revision(), Revision::new(2));

    for value in [
        ItemValueV1::list(Vec::new()).unwrap(),
        ItemValueV1::list(vec!["one".to_owned(), "two".to_owned()]).unwrap(),
    ] {
        assert_eq!(
            ItemSlotV1::new(ItemSlotInputV1 {
                attempt_id: attempt_id.clone(),
                specification: &list,
                revision: Revision::new(1),
                value: Some(value),
                created_at: Some(UnixMillis::new(100)),
                updated_at: Some(UnixMillis::new(100)),
            })
            .unwrap()
            .revision(),
            Revision::new(1)
        );
    }
}
#[test]
fn item_slot_hydration_rejects_invalid_metadata_and_singleton_revisions() {
    let attempt_id = AttemptId::new(UUID_B).unwrap();
    let list = ItemSpecV1::list(common("list", false), 0, 3, 8, true).unwrap();
    assert_invalid_state(
        ItemSlotV1::new(ItemSlotInputV1 {
            attempt_id: attempt_id.clone(),
            specification: &list,
            revision: Revision::new(1),
            value: Some(ItemValueV1::list(vec!["one".to_owned()]).unwrap()),
            created_at: Some(UnixMillis::new(100)),
            updated_at: Some(UnixMillis::new(101)),
        }),
        "invalid populated item slot metadata",
    );

    let confirm = ItemSpecV1::confirm(common("confirm", false));
    assert!(
        ItemSlotV1::new(ItemSlotInputV1 {
            attempt_id: attempt_id.clone(),
            specification: &confirm,
            revision: Revision::new(1),
            value: None,
            created_at: Some(UnixMillis::new(100)),
            updated_at: Some(UnixMillis::new(101)),
        })
        .is_err()
    );
    assert!(
        ItemSlotV1::new(ItemSlotInputV1 {
            attempt_id: attempt_id.clone(),
            specification: &confirm,
            revision: Revision::new(2),
            value: Some(ItemValueV1::confirm()),
            created_at: Some(UnixMillis::new(100)),
            updated_at: Some(UnixMillis::new(102)),
        })
        .is_err()
    );

    let choice =
        ItemSpecV1::choice(common("choice-single", false), vec!["only".to_owned()]).unwrap();
    assert!(
        ItemSlotV1::new(ItemSlotInputV1 {
            attempt_id: attempt_id.clone(),
            specification: &choice,
            revision: Revision::new(2),
            value: Some(ItemValueV1::choice("only").unwrap()),
            created_at: Some(UnixMillis::new(100)),
            updated_at: Some(UnixMillis::new(102)),
        })
        .is_err()
    );

    let integer = ItemSpecV1::integer(common("integer-single", false), Some(7), Some(7)).unwrap();
    assert!(
        ItemSlotV1::new(ItemSlotInputV1 {
            attempt_id: attempt_id.clone(),
            specification: &integer,
            revision: Revision::new(2),
            value: Some(ItemValueV1::integer(7)),
            created_at: Some(UnixMillis::new(100)),
            updated_at: Some(UnixMillis::new(102)),
        })
        .is_err()
    );
    let minimum_only =
        ItemSpecV1::integer(common("integer-minimum-only", false), Some(i64::MAX), None).unwrap();
    let maximum_only =
        ItemSpecV1::integer(common("integer-maximum-only", false), None, Some(i64::MIN)).unwrap();
    for (specification, value) in [(&minimum_only, i64::MAX), (&maximum_only, i64::MIN)] {
        assert!(
            ItemSlotV1::new(ItemSlotInputV1 {
                attempt_id: attempt_id.clone(),
                specification,
                revision: Revision::new(2),
                value: Some(ItemValueV1::integer(value)),
                created_at: Some(UnixMillis::new(100)),
                updated_at: Some(UnixMillis::new(102)),
            })
            .is_err()
        );
    }
}

#[test]
fn text_storage_preserves_original_values_while_satisfaction_uses_trimmed_bounds() {
    let text = ItemSpecV1::text(common("text", true), 2, 3, true).unwrap();
    let list = ItemSpecV1::list(common("list", true), 2, 3, 8, true).unwrap();
    let attempt_id = AttemptId::new(UUID_B).unwrap();

    let short_text = ItemSlotV1::new_empty(attempt_id.clone(), &text)
        .with_value(&text, ItemValueV1::text(" a "), UnixMillis::new(20))
        .unwrap();
    assert_eq!(
        short_text.value().and_then(ItemValueV1::as_text),
        Some(" a ")
    );
    assert!(!item_satisfied(&text, short_text.value()));
    let text_slot = ItemSlotV1::new_empty(attempt_id.clone(), &text)
        .with_value(&text, ItemValueV1::text(" abc "), UnixMillis::new(20))
        .unwrap();
    assert_eq!(
        text_slot.value().and_then(ItemValueV1::as_text),
        Some(" abc ")
    );
    assert!(item_satisfied(&text, text_slot.value()));
    assert!(
        ItemSlotV1::new_empty(attempt_id.clone(), &text)
            .with_value(&text, ItemValueV1::text("four"), UnixMillis::new(20),)
            .is_err()
    );
    assert!(
        ItemSlotV1::new_empty(attempt_id.clone(), &text)
            .with_value(
                &text,
                ItemValueV1::text("x".repeat(MAX_TEXT_LENGTH as usize + 1)),
                UnixMillis::new(20),
            )
            .is_err()
    );

    let partial_list = ItemSlotV1::new_empty(attempt_id.clone(), &list)
        .with_value(
            &list,
            ItemValueV1::list(vec!["first".to_owned()]).unwrap(),
            UnixMillis::new(20),
        )
        .unwrap();
    assert_eq!(
        partial_list.value().and_then(ItemValueV1::as_list),
        Some(["first".to_owned()].as_slice())
    );
    assert!(!item_satisfied(&list, partial_list.value()));
    let maximum_multibyte_list_entry = "界".repeat(8);
    assert!(
        ItemSlotV1::new_empty(attempt_id.clone(), &list)
            .with_value(
                &list,
                ItemValueV1::list(vec![maximum_multibyte_list_entry]).unwrap(),
                UnixMillis::new(20),
            )
            .is_ok()
    );
    assert_eq!(
        ItemSlotV1::new_empty(attempt_id.clone(), &list)
            .with_value(
                &list,
                ItemValueV1::list(vec!["界".repeat(9)]).unwrap(),
                UnixMillis::new(20),
            )
            .unwrap_err(),
        DomainError::InvalidState {
            reason: "item value does not meet its storage rules",
        }
    );
    assert!(
        ItemSlotV1::new_empty(attempt_id.clone(), &list)
            .with_value(
                &list,
                ItemValueV1::list(vec!["first".to_owned(), "first".to_owned()]).unwrap(),
                UnixMillis::new(20),
            )
            .is_err()
    );
    assert!(
        ItemSlotV1::new_empty(attempt_id.clone(), &list)
            .with_value(
                &list,
                ItemValueV1::list(vec!["123456789".to_owned()]).unwrap(),
                UnixMillis::new(20),
            )
            .is_err()
    );
    assert!(ItemValueV1::list(vec![" ".to_owned()]).is_err());
    assert!(
        ItemSlotV1::new_empty(attempt_id, &list)
            .with_value(
                &list,
                ItemValueV1::list(vec![
                    "first".to_owned(),
                    "second".to_owned(),
                    "third".to_owned(),
                    "fourth".to_owned(),
                ])
                .unwrap(),
                UnixMillis::new(20),
            )
            .is_err()
    );
}
#[test]
fn integer_and_global_text_storage_boundaries_are_exact() {
    let attempt_id = AttemptId::new(UUID_C).unwrap();
    let integer =
        ItemSpecV1::integer(common("integer-boundary", false), Some(-7), Some(7)).unwrap();

    for value in [-7, 7] {
        assert!(
            ItemSlotV1::new_empty(attempt_id.clone(), &integer)
                .with_value(&integer, ItemValueV1::integer(value), UnixMillis::new(20),)
                .is_ok()
        );
    }
    for value in [-8, 8] {
        assert_eq!(
            ItemSlotV1::new_empty(attempt_id.clone(), &integer)
                .with_value(&integer, ItemValueV1::integer(value), UnixMillis::new(20),)
                .unwrap_err(),
            DomainError::InvalidState {
                reason: "item value does not meet its storage rules",
            }
        );
    }

    let text = ItemSpecV1::text(
        common("global-text-boundary", false),
        0,
        MAX_TEXT_LENGTH,
        true,
    )
    .unwrap();
    let at_global_maximum = "x".repeat(MAX_TEXT_LENGTH as usize);
    let slot = ItemSlotV1::new_empty(attempt_id.clone(), &text)
        .with_value(
            &text,
            ItemValueV1::text(at_global_maximum.as_str()),
            UnixMillis::new(30),
        )
        .unwrap();
    assert_eq!(
        slot.value().and_then(ItemValueV1::as_text),
        Some(at_global_maximum.as_str())
    );
    let multibyte_at_global_maximum = "界".repeat(MAX_TEXT_LENGTH as usize);
    assert!(
        ItemSlotV1::new_empty(attempt_id.clone(), &text)
            .with_value(
                &text,
                ItemValueV1::text(multibyte_at_global_maximum),
                UnixMillis::new(30),
            )
            .is_ok()
    );
    assert_eq!(
        ItemSlotV1::new_empty(attempt_id.clone(), &text)
            .with_value(
                &text,
                ItemValueV1::text("界".repeat(MAX_TEXT_LENGTH as usize + 1)),
                UnixMillis::new(30),
            )
            .unwrap_err(),
        DomainError::InvalidState {
            reason: "item value does not meet its storage rules",
        }
    );
    assert_eq!(
        ItemSlotV1::new_empty(attempt_id, &text)
            .with_value(
                &text,
                ItemValueV1::text("x".repeat(MAX_TEXT_LENGTH as usize + 1)),
                UnixMillis::new(30),
            )
            .unwrap_err(),
        DomainError::InvalidState {
            reason: "item value does not meet its storage rules",
        }
    );
}

#[test]
fn optional_values_do_not_block_required_readiness_and_wrong_values_are_not_stored() {
    let required = ItemSpecV1::confirm(common("required", true));
    let optional = ItemSpecV1::text(common("optional", false), 0, 5, true).unwrap();
    let stage = StageSpecV1::new(
        stage_id("stage"),
        "Stage",
        Vec::new(),
        vec![required.clone(), optional.clone()],
        SkipPolicyV1::not_allowed(),
    )
    .unwrap();
    let attempt_id = AttemptId::new(UUID_B).unwrap();
    let required_slot = ItemSlotV1::new_empty(attempt_id.clone(), &required);
    let optional_slot = ItemSlotV1::new_empty(attempt_id, &optional);

    assert!(!required_items_satisfied(
        &stage,
        &[required_slot.clone(), optional_slot.clone()]
    ));
    let required_slot = required_slot
        .with_value(&required, ItemValueV1::confirm(), UnixMillis::new(20))
        .unwrap();
    assert!(required_items_satisfied(
        &stage,
        &[required_slot.clone(), optional_slot]
    ));
    assert!(
        required_slot
            .with_value(
                &required,
                ItemValueV1::text("wrong type"),
                UnixMillis::new(21),
            )
            .is_err()
    );
}

#[test]
fn list_capacity_allows_required_completion_at_the_one_item_boundary() {
    for required in [false, true] {
        assert_eq!(
            ItemSpecV1::list(common("zero-capacity", required), 0, 0, 8, true).unwrap_err(),
            DomainError::InvalidState {
                reason: "list must allow at least one item",
            }
        );
    }

    let list = ItemSpecV1::list(common("single-capacity", true), 1, 1, 8, true).unwrap();
    let stage = StageSpecV1::new(
        stage_id("list-stage"),
        "List stage",
        Vec::new(),
        vec![list.clone()],
        SkipPolicyV1::not_allowed(),
    )
    .unwrap();
    let attempt_id = AttemptId::new(UUID_B).unwrap();
    let empty_slot = ItemSlotV1::new_empty(attempt_id, &list);

    assert!(!required_items_satisfied(
        &stage,
        std::slice::from_ref(&empty_slot),
    ));

    let completed_slot = empty_slot
        .with_value(
            &list,
            ItemValueV1::list(vec!["only".to_owned()]).unwrap(),
            UnixMillis::new(20),
        )
        .unwrap();
    assert!(item_satisfied(&list, completed_slot.value()));
    assert!(required_items_satisfied(&stage, &[completed_slot]));
}

#[test]
fn artifact_metadata_rejects_unsafe_paths_and_requires_exact_media_types() {
    assert!(ArtifactValueV1::local_path("/tmp/a", digest(), 1, "text/plain").is_err());
    assert!(ArtifactValueV1::local_path("../outside", digest(), 1, "text/plain").is_err());
    assert!(ArtifactValueV1::local_path("dir/../outside", digest(), 1, "text/plain").is_err());
    assert!(ArtifactValueV1::local_path("safe/file.txt", digest(), 1, "Text/plain").is_err());
    assert!(ArtifactValueV1::external_reference("", digest(), 1, "text/plain").is_err());
    let maximum_local_path = "a".repeat(4_000);
    let rehydrated_local_artifact =
        ArtifactValueV1::local_path(maximum_local_path, digest(), 1, "text/plain").unwrap();
    assert_eq!(rehydrated_local_artifact.location().chars().count(), 4_000);
    let maximum_multibyte_local_path = "界".repeat(4_000);
    let multibyte_local_artifact =
        ArtifactValueV1::local_path(maximum_multibyte_local_path, digest(), 1, "text/plain")
            .unwrap();
    assert_eq!(multibyte_local_artifact.location().chars().count(), 4_000);
    assert_invalid_state(
        ArtifactValueV1::local_path("界".repeat(4_001), digest(), 1, "text/plain"),
        "local artifact path must contain at most 4000 scalars",
    );
    let local_path_specification =
        ItemSpecV1::artifact(common("local-path", false), vec!["text/plain".to_owned()]).unwrap();
    let rehydrated_local_slot = ItemSlotV1::new(ItemSlotInputV1 {
        attempt_id: AttemptId::new(UUID_C).unwrap(),
        specification: &local_path_specification,
        revision: Revision::new(1),
        value: Some(ItemValueV1::artifact(rehydrated_local_artifact)),
        created_at: Some(UnixMillis::new(20)),
        updated_at: Some(UnixMillis::new(20)),
    })
    .unwrap();
    assert_eq!(
        rehydrated_local_slot
            .value()
            .unwrap()
            .as_artifact()
            .unwrap()
            .location()
            .chars()
            .count(),
        4_000
    );
    assert_invalid_state(
        ArtifactValueV1::local_path("a".repeat(4_001), digest(), 1, "text/plain"),
        "local artifact path must contain at most 4000 scalars",
    );

    let specification =
        ItemSpecV1::artifact(common("report", true), vec!["application/json".to_owned()]).unwrap();
    let reference =
        ArtifactValueV1::external_reference("urn:example:report", digest(), 42, "text/plain")
            .unwrap();
    assert_eq!(reference.location(), "urn:example:report");
    assert!(
        ItemSlotV1::new_empty(AttemptId::new(UUID_C).unwrap(), &specification)
            .with_value(
                &specification,
                ItemValueV1::artifact(reference.clone()),
                UnixMillis::new(20),
            )
            .is_err()
    );
    assert!(!item_satisfied(
        &specification,
        Some(&ItemValueV1::artifact(reference))
    ));
}

#[test]
fn snapshot_and_aggregate_construction_reject_duplicates_ordering_and_cursor_mismatches() {
    let duplicate_item = ItemSpecV1::confirm(common("same", true));
    assert!(
        StageSpecV1::new(
            stage_id("duplicate-items"),
            "Duplicate items",
            Vec::new(),
            vec![duplicate_item.clone(), duplicate_item],
            SkipPolicyV1::not_allowed(),
        )
        .is_err()
    );

    let first = StageSpecV1::new(
        stage_id("first"),
        "First",
        Vec::new(),
        vec![ItemSpecV1::confirm(common("first-confirm", true))],
        SkipPolicyV1::not_allowed(),
    )
    .unwrap();
    let second = StageSpecV1::new(
        stage_id("second"),
        "Second",
        Vec::new(),
        vec![ItemSpecV1::confirm(common("second-confirm", true))],
        SkipPolicyV1::not_allowed(),
    )
    .unwrap();
    assert!(
        ProcedureSnapshotV1::assemble(ProcedureSnapshotAssemblyInputV1 {
            snapshot_id: ProcedureSnapshotId::new(UUID_A).unwrap(),
            procedure_id: "duplicate-stage-procedure".to_owned(),
            procedure_version: "1".to_owned(),
            name: "Duplicate stage procedure".to_owned(),
            description: None,
            stages: vec![first.clone(), first.clone()],
            return_policy: ReturnPolicyV1::any_previous(),
            source_label: ProcedureSourceLabelV1::new("preset: duplicate-stage-procedure").unwrap(),
            accepted_warning_codes: vec![ProcedureWarningCodeV1::AnyPreviousReturnPolicy],
            created_at: UnixMillis::new(10),
        })
        .is_err()
    );
    assert!(
        ProcedureSnapshotV1::assemble(ProcedureSnapshotAssemblyInputV1 {
            snapshot_id: ProcedureSnapshotId::new(UUID_A).unwrap(),
            procedure_id: "unknown-return-procedure".to_owned(),
            procedure_version: "1".to_owned(),
            name: "Unknown return procedure".to_owned(),
            description: None,
            stages: vec![first.clone(), second.clone()],
            return_policy: ReturnPolicyV1::only(vec![stage_id("missing")]).unwrap(),
            source_label: ProcedureSourceLabelV1::new("preset: unknown-return-procedure").unwrap(),
            accepted_warning_codes: Vec::new(),
            created_at: UnixMillis::new(10),
        })
        .is_err()
    );

    let snapshot = procedure_snapshot(
        vec![first.clone(), second.clone()],
        vec![ProcedureWarningCodeV1::AnyPreviousReturnPolicy],
    );
    let session_id = SessionId::new(UUID_B).unwrap();
    let attempt = AttemptV1::fresh(
        AttemptId::new(UUID_C).unwrap(),
        session_id.clone(),
        &first,
        1,
        UnixMillis::new(10),
    )
    .unwrap();
    assert_eq!(attempt.item_slots().len(), first.items().len());
    assert!(
        attempt
            .item_slots()
            .iter()
            .all(|slot| slot.value().is_none())
    );
    assert!(attempt.blockers().is_empty());
    let current =
        StageProgressV1::current(first.id().clone(), 0, attempt.attempt_id().clone(), 1).unwrap();
    let pending = StageProgressV1::pending(second.id().clone(), 1);
    assert!(
        SessionAggregateV1::new(SessionAggregateInputV1 {
            session_id: session_id.clone(),
            task_title: "Task".to_owned(),
            snapshot: snapshot.clone(),
            lifecycle: SessionLifecycle::Running,
            revision: Revision::new(1),
            stage_progress: vec![current.clone(), pending.clone()],
            attempts: vec![attempt.clone()],
            active_stage_id: Some(second.id().clone()),
            active_attempt_id: Some(attempt.attempt_id().clone()),
            created_at: UnixMillis::new(10),
            completed_at: None,
            cancelled_at: None,
            cancel_reason: None,
        })
        .is_err()
    );
    assert!(
        SessionAggregateV1::new(SessionAggregateInputV1 {
            session_id,
            task_title: "Task".to_owned(),
            snapshot,
            lifecycle: SessionLifecycle::Running,
            revision: Revision::new(1),
            stage_progress: vec![StageProgressV1::pending(second.id().clone(), 0), current],
            attempts: vec![attempt],
            active_stage_id: Some(first.id().clone()),
            active_attempt_id: Some(AttemptId::new(UUID_C).unwrap()),
            created_at: UnixMillis::new(10),
            completed_at: None,
            cancelled_at: None,
            cancel_reason: None,
        })
        .is_err()
    );

    let blocker = BlockerV1::open(
        BlockerId::new(UUID_A).unwrap(),
        AttemptId::new(UUID_C).unwrap(),
        "blocked",
        UnixMillis::new(10),
    )
    .unwrap();
    assert!(blocker.resolve(UnixMillis::new(9)).is_err());
}

#[test]
fn attempt_blocker_limit_bounds_compact_status_projection() {
    let stage = StageSpecV1::new(
        stage_id("bounded-blockers"),
        "Bounded blockers",
        Vec::new(),
        Vec::new(),
        SkipPolicyV1::not_allowed(),
    )
    .unwrap();
    let session_id = SessionId::new(UUID_B).unwrap();
    let attempt_id = AttemptId::new(UUID_C).unwrap();
    let blockers = (0..=MAX_OPEN_BLOCKERS_PER_ATTEMPT_V1)
        .map(|index| {
            BlockerV1::open(
                BlockerId::new(format!("00000000-0000-4000-8000-{index:012x}")).unwrap(),
                attempt_id.clone(),
                "blocked",
                UnixMillis::new(10),
            )
            .unwrap()
        })
        .collect::<Vec<_>>();

    assert!(
        hydrated_attempt(
            UUID_C,
            &session_id,
            &stage,
            1,
            AttemptLifecycle::Active,
            10,
            None,
            None,
            blockers[..MAX_OPEN_BLOCKERS_PER_ATTEMPT_V1].to_vec(),
        )
        .is_ok()
    );
    assert_eq!(
        hydrated_attempt(
            UUID_C,
            &session_id,
            &stage,
            1,
            AttemptLifecycle::Active,
            10,
            None,
            None,
            blockers.clone(),
        ),
        Err(DomainError::BlockerLimitReached {
            maximum_open_blockers: MAX_OPEN_BLOCKERS_PER_ATTEMPT_V1,
        }),
    );

    let resolved = BlockerV1::open(
        BlockerId::new("00000000-0000-4000-8000-ffffffffffff").unwrap(),
        attempt_id,
        "resolved",
        UnixMillis::new(10),
    )
    .unwrap()
    .resolve(UnixMillis::new(11))
    .unwrap();
    let mut bounded_with_history = blockers[..MAX_OPEN_BLOCKERS_PER_ATTEMPT_V1].to_vec();
    bounded_with_history.push(resolved);
    assert!(
        hydrated_attempt(
            UUID_C,
            &session_id,
            &stage,
            1,
            AttemptLifecycle::Active,
            10,
            None,
            None,
            bounded_with_history,
        )
        .is_ok()
    );
}

#[test]
fn terminal_attempts_are_stage_aware_and_only_skip_or_abandon_close_blockers() {
    let required = ItemSpecV1::confirm(common("required", true));
    let stage = StageSpecV1::new(
        stage_id("stage"),
        "Stage",
        Vec::new(),
        vec![required.clone()],
        SkipPolicyV1::allowed(true),
    )
    .unwrap();
    let other_stage = StageSpecV1::new(
        stage_id("other"),
        "Other",
        Vec::new(),
        Vec::new(),
        SkipPolicyV1::not_allowed(),
    )
    .unwrap();
    let session_id = SessionId::new(UUID_B).unwrap();
    let attempt_id = AttemptId::new(UUID_C).unwrap();
    let empty = ItemSlotV1::new_empty(attempt_id.clone(), &required);
    assert!(
        AttemptV1::new(AttemptInputV1 {
            attempt_id: attempt_id.clone(),
            session_id: session_id.clone(),
            stage: &stage,
            number: 1,
            lifecycle: AttemptLifecycle::Completed,
            started_at: UnixMillis::new(10),
            ended_at: Some(UnixMillis::new(20)),
            reason: None,
            item_slots: vec![empty],
            blockers: Vec::new(),
        })
        .is_err()
    );
    let filled = ItemSlotV1::new_empty(attempt_id.clone(), &required)
        .with_value(&required, ItemValueV1::confirm(), UnixMillis::new(12))
        .unwrap();
    let active = AttemptV1::new(AttemptInputV1 {
        attempt_id: attempt_id.clone(),
        session_id: session_id.clone(),
        stage: &stage,
        number: 1,
        lifecycle: AttemptLifecycle::Active,
        started_at: UnixMillis::new(10),
        ended_at: None,
        reason: None,
        item_slots: vec![filled.clone()],
        blockers: Vec::new(),
    })
    .unwrap();
    assert_eq!(
        active.with_replaced_slot(&stage, filled.clone()).unwrap(),
        active
    );
    let cleared = filled.with_cleared(&required, UnixMillis::new(13)).unwrap();
    assert_eq!(
        active
            .with_replaced_slot(&stage, cleared.clone())
            .unwrap()
            .item_slots()[0]
            .revision(),
        Revision::new(2)
    );
    assert_eq!(cleared.created_at(), Some(UnixMillis::new(12)));
    assert_eq!(cleared.updated_at(), Some(UnixMillis::new(13)));
    let rollback = ItemSlotV1::new(ItemSlotInputV1 {
        attempt_id: attempt_id.clone(),
        specification: &required,
        revision: Revision::ZERO,
        value: None,
        created_at: None,
        updated_at: None,
    })
    .unwrap();
    assert!(active.with_replaced_slot(&stage, rollback).is_err());
    let jumped = ItemSlotV1::new(ItemSlotInputV1 {
        attempt_id: attempt_id.clone(),
        specification: &required,
        revision: Revision::new(3),
        value: Some(ItemValueV1::confirm()),
        created_at: Some(UnixMillis::new(12)),
        updated_at: Some(UnixMillis::new(12)),
    })
    .unwrap();
    assert!(active.with_replaced_slot(&stage, jumped).is_err());
    assert!(
        active
            .with_replaced_slot(&other_stage, filled.clone())
            .is_err()
    );
    assert!(
        active
            .with_terminal(
                &other_stage,
                AttemptLifecycle::Completed,
                UnixMillis::new(20),
                None,
            )
            .is_err()
    );
    assert_eq!(
        active
            .with_terminal(
                &stage,
                AttemptLifecycle::Completed,
                UnixMillis::new(20),
                None
            )
            .unwrap()
            .lifecycle(),
        AttemptLifecycle::Completed
    );
    let blocker = BlockerV1::open(
        BlockerId::new(UUID_A).unwrap(),
        attempt_id.clone(),
        "waiting",
        UnixMillis::new(13),
    )
    .unwrap();
    let blocked = active.with_added_blocker(blocker).unwrap();
    assert!(
        blocked
            .with_terminal(
                &stage,
                AttemptLifecycle::Completed,
                UnixMillis::new(20),
                None
            )
            .is_err()
    );
    let skipped = blocked
        .with_terminal(
            &stage,
            AttemptLifecycle::Skipped,
            UnixMillis::new(20),
            Some("not needed".to_owned()),
        )
        .unwrap();
    assert_eq!(
        skipped.blockers()[0].resolved_at(),
        Some(UnixMillis::new(20))
    );
    assert_eq!(skipped.blockers()[0].state(), BlockerState::Resolved);
    let abandoned = blocked
        .with_terminal(
            &stage,
            AttemptLifecycle::Abandoned,
            UnixMillis::new(21),
            Some("retry later".to_owned()),
        )
        .unwrap();
    assert_eq!(
        abandoned.blockers()[0].resolved_at(),
        Some(UnixMillis::new(21))
    );
    assert_eq!(abandoned.blockers()[0].state(), BlockerState::Resolved);
    assert!(
        AttemptV1::new(AttemptInputV1 {
            attempt_id,
            session_id,
            stage: &stage,
            number: 1,
            lifecycle: AttemptLifecycle::Skipped,
            started_at: UnixMillis::new(10),
            ended_at: Some(UnixMillis::new(20)),
            reason: None,
            item_slots: vec![filled],
            blockers: Vec::new(),
        })
        .is_err()
    );
    assert!(
        AttemptV1::new(AttemptInputV1 {
            attempt_id: AttemptId::new(UUID_C).unwrap(),
            session_id: SessionId::new(UUID_B).unwrap(),
            stage: &other_stage,
            number: 1,
            lifecycle: AttemptLifecycle::Skipped,
            started_at: UnixMillis::new(10),
            ended_at: Some(UnixMillis::new(20)),
            reason: Some("not permitted".to_owned()),
            item_slots: Vec::new(),
            blockers: Vec::new(),
        })
        .is_err()
    );
    assert!(
        AttemptV1::new(AttemptInputV1 {
            attempt_id: AttemptId::new(UUID_C).unwrap(),
            session_id: SessionId::new(UUID_B).unwrap(),
            stage: &other_stage,
            number: 1,
            lifecycle: AttemptLifecycle::Abandoned,
            started_at: UnixMillis::new(10),
            ended_at: Some(UnixMillis::new(20)),
            reason: None,
            item_slots: Vec::new(),
            blockers: Vec::new(),
        })
        .is_err()
    );
}
#[test]
fn slot_replacements_must_match_public_single_step_mutations() {
    let item = ItemSpecV1::text(common("text", false), 0, 16, true).unwrap();
    let stage = StageSpecV1::new(
        stage_id("replacement-stage"),
        "Replacement stage",
        Vec::new(),
        vec![item.clone()],
        SkipPolicyV1::not_allowed(),
    )
    .unwrap();
    let session_id = SessionId::new(UUID_B).unwrap();
    let attempt_id = AttemptId::new(UUID_C).unwrap();
    let populated = ItemSlotV1::new_empty(attempt_id.clone(), &item)
        .with_value(&item, ItemValueV1::text("first"), UnixMillis::new(12))
        .unwrap();
    let active = hydrated_active_attempt(&attempt_id, &session_id, &stage, vec![populated.clone()]);

    let changed = populated
        .with_value(&item, ItemValueV1::text("second"), UnixMillis::new(13))
        .unwrap();
    let updated = active.with_replaced_slot(&stage, changed.clone()).unwrap();
    assert_eq!(updated.item_slots()[0], changed);

    let same_value_bump = ItemSlotV1::new(ItemSlotInputV1 {
        attempt_id: attempt_id.clone(),
        specification: &item,
        revision: Revision::new(2),
        value: Some(ItemValueV1::text("first")),
        created_at: Some(UnixMillis::new(12)),
        updated_at: Some(UnixMillis::new(13)),
    })
    .unwrap();
    assert_invalid_state(
        active.with_replaced_slot(&stage, same_value_bump),
        "replacement item slot is not reachable in one transition",
    );

    let changed_creation_time = ItemSlotV1::new(ItemSlotInputV1 {
        attempt_id: attempt_id.clone(),
        specification: &item,
        revision: Revision::new(2),
        value: Some(ItemValueV1::text("second")),
        created_at: Some(UnixMillis::new(13)),
        updated_at: Some(UnixMillis::new(13)),
    })
    .unwrap();
    assert_invalid_state(
        active.with_replaced_slot(&stage, changed_creation_time),
        "replacement item slot is not reachable in one transition",
    );

    let cleared = populated.with_cleared(&item, UnixMillis::new(13)).unwrap();
    let active_tombstone = hydrated_active_attempt(&attempt_id, &session_id, &stage, vec![cleared]);
    let tombstone_bump = ItemSlotV1::new(ItemSlotInputV1 {
        attempt_id: attempt_id.clone(),
        specification: &item,
        revision: Revision::new(3),
        value: None,
        created_at: Some(UnixMillis::new(12)),
        updated_at: Some(UnixMillis::new(14)),
    })
    .unwrap();
    assert_invalid_state(
        active_tombstone.with_replaced_slot(&stage, tombstone_bump),
        "replacement item slot is not reachable in one transition",
    );

    let stale_repopulation = ItemSlotV1::new(ItemSlotInputV1 {
        attempt_id,
        specification: &item,
        revision: Revision::new(3),
        value: Some(ItemValueV1::text("third")),
        created_at: Some(UnixMillis::new(12)),
        updated_at: Some(UnixMillis::new(12)),
    })
    .unwrap();
    assert_invalid_state(
        active_tombstone.with_replaced_slot(&stage, stale_repopulation),
        "item value timestamp precedes its current update",
    );
}
#[test]
fn first_active_or_completed_attempts_cannot_retain_reopen_reasons() {
    let stage = StageSpecV1::new(
        stage_id("attempt-reasons"),
        "Attempt reasons",
        Vec::new(),
        Vec::new(),
        SkipPolicyV1::allowed(true),
    )
    .unwrap();
    let session_id = SessionId::new(UUID_B).unwrap();

    assert_invalid_state(
        hydrated_attempt(
            UUID_A,
            &session_id,
            &stage,
            1,
            AttemptLifecycle::Active,
            10,
            None,
            Some("impossible reopen"),
            Vec::new(),
        ),
        "first active or completed attempts cannot retain a reason",
    );
    assert_invalid_state(
        hydrated_attempt(
            UUID_A,
            &session_id,
            &stage,
            1,
            AttemptLifecycle::Completed,
            10,
            Some(20),
            Some("impossible reopen"),
            Vec::new(),
        ),
        "first active or completed attempts cannot retain a reason",
    );
    assert!(
        hydrated_attempt(
            UUID_A,
            &session_id,
            &stage,
            1,
            AttemptLifecycle::Skipped,
            10,
            Some(20),
            Some("legitimate skip"),
            Vec::new(),
        )
        .is_ok()
    );
    assert!(
        hydrated_attempt(
            UUID_A,
            &session_id,
            &stage,
            1,
            AttemptLifecycle::Abandoned,
            10,
            Some(20),
            Some("legitimate abandonment"),
            Vec::new(),
        )
        .is_ok()
    );
}
#[test]
fn aggregate_history_revalidates_authoritative_terminal_stage_rules() {
    let required = ItemSpecV1::confirm(common("required", true));
    let authoritative_completed = StageSpecV1::new(
        stage_id("strict-completion"),
        "Strict completion",
        Vec::new(),
        vec![required],
        SkipPolicyV1::not_allowed(),
    )
    .unwrap();
    let weaker_completed = StageSpecV1::new(
        stage_id("strict-completion"),
        "Weaker completion",
        Vec::new(),
        vec![ItemSpecV1::confirm(common("required", false))],
        SkipPolicyV1::not_allowed(),
    )
    .unwrap();
    let session_id = SessionId::new(UUID_B).unwrap();
    let completed = plain_attempt(PlainAttemptFixture {
        attempt: UUID_A,
        session_id: &session_id,
        stage: &weaker_completed,
        number: 1,
        lifecycle: AttemptLifecycle::Completed,
        started_at: 10,
        ended_at: Some(20),
        reason: None,
    });
    assert!(
        SessionAggregateV1::new(SessionAggregateInputV1 {
            session_id: session_id.clone(),
            task_title: "Task".to_owned(),
            snapshot: procedure_snapshot(
                vec![authoritative_completed.clone()],
                vec![ProcedureWarningCodeV1::AnyPreviousReturnPolicy],
            ),
            lifecycle: SessionLifecycle::Completed,
            revision: Revision::new(2),
            stage_progress: vec![
                StageProgressV1::new(
                    authoritative_completed.id().clone(),
                    0,
                    StageProgressState::Done,
                    1,
                    Some(completed.attempt_id().clone()),
                )
                .unwrap(),
            ],
            attempts: vec![completed],
            active_stage_id: None,
            active_attempt_id: None,
            created_at: UnixMillis::new(10),
            completed_at: Some(UnixMillis::new(20)),
            cancelled_at: None,
            cancel_reason: None,
        })
        .is_err()
    );

    let authoritative_skipped = StageSpecV1::new(
        stage_id("strict-skip"),
        "Strict skip",
        Vec::new(),
        Vec::new(),
        SkipPolicyV1::not_allowed(),
    )
    .unwrap();
    let weaker_skipped = StageSpecV1::new(
        stage_id("strict-skip"),
        "Weaker skip",
        Vec::new(),
        Vec::new(),
        SkipPolicyV1::allowed(false),
    )
    .unwrap();
    let skipped = plain_attempt(PlainAttemptFixture {
        attempt: UUID_C,
        session_id: &session_id,
        stage: &weaker_skipped,
        number: 1,
        lifecycle: AttemptLifecycle::Skipped,
        started_at: 10,
        ended_at: Some(20),
        reason: None,
    });
    assert!(
        SessionAggregateV1::new(SessionAggregateInputV1 {
            session_id,
            task_title: "Task".to_owned(),
            snapshot: procedure_snapshot(
                vec![authoritative_skipped.clone()],
                vec![
                    ProcedureWarningCodeV1::StageHasNoRequiredItems,
                    ProcedureWarningCodeV1::AnyPreviousReturnPolicy,
                ],
            ),
            lifecycle: SessionLifecycle::Completed,
            revision: Revision::new(2),
            stage_progress: vec![
                StageProgressV1::new(
                    authoritative_skipped.id().clone(),
                    0,
                    StageProgressState::Skipped,
                    1,
                    Some(skipped.attempt_id().clone()),
                )
                .unwrap(),
            ],
            attempts: vec![skipped],
            active_stage_id: None,
            active_attempt_id: None,
            created_at: UnixMillis::new(10),
            completed_at: Some(UnixMillis::new(20)),
            cancelled_at: None,
            cancel_reason: None,
        })
        .is_err()
    );
}

#[test]
fn attempt_chronology_rejects_out_of_lifetime_records() {
    let item = ItemSpecV1::confirm(common("item", true));
    let stage = StageSpecV1::new(
        stage_id("chronology"),
        "Chronology",
        Vec::new(),
        vec![item.clone()],
        SkipPolicyV1::allowed(false),
    )
    .unwrap();
    let session_id = SessionId::new(UUID_B).unwrap();
    let attempt_id = AttemptId::new(UUID_C).unwrap();
    let early_slot = ItemSlotV1::new(ItemSlotInputV1 {
        attempt_id: attempt_id.clone(),
        specification: &item,
        revision: Revision::new(1),
        value: Some(ItemValueV1::confirm()),
        created_at: Some(UnixMillis::new(9)),
        updated_at: Some(UnixMillis::new(9)),
    })
    .unwrap();
    assert!(
        AttemptV1::new(AttemptInputV1 {
            attempt_id: attempt_id.clone(),
            session_id: session_id.clone(),
            stage: &stage,
            number: 1,
            lifecycle: AttemptLifecycle::Active,
            started_at: UnixMillis::new(10),
            ended_at: None,
            reason: None,
            item_slots: vec![early_slot],
            blockers: Vec::new(),
        })
        .is_err()
    );
    let slot = ItemSlotV1::new_empty(attempt_id.clone(), &item)
        .with_value(&item, ItemValueV1::confirm(), UnixMillis::new(11))
        .unwrap();
    let early_blocker = BlockerV1::open(
        BlockerId::new(UUID_A).unwrap(),
        attempt_id.clone(),
        "early",
        UnixMillis::new(9),
    )
    .unwrap();
    assert!(
        AttemptV1::new(AttemptInputV1 {
            attempt_id: attempt_id.clone(),
            session_id: session_id.clone(),
            stage: &stage,
            number: 1,
            lifecycle: AttemptLifecycle::Active,
            started_at: UnixMillis::new(10),
            ended_at: None,
            reason: None,
            item_slots: vec![slot.clone()],
            blockers: vec![early_blocker],
        })
        .is_err()
    );
    let late_slot = ItemSlotV1::new(ItemSlotInputV1 {
        attempt_id: attempt_id.clone(),
        specification: &item,
        revision: Revision::new(3),
        value: Some(ItemValueV1::confirm()),
        created_at: Some(UnixMillis::new(11)),
        updated_at: Some(UnixMillis::new(21)),
    })
    .unwrap();
    assert!(
        AttemptV1::new(AttemptInputV1 {
            attempt_id: attempt_id.clone(),
            session_id: session_id.clone(),
            stage: &stage,
            number: 1,
            lifecycle: AttemptLifecycle::Skipped,
            started_at: UnixMillis::new(10),
            ended_at: Some(UnixMillis::new(20)),
            reason: None,
            item_slots: vec![late_slot],
            blockers: Vec::new(),
        })
        .is_err()
    );
    let late_blocker = BlockerV1::open(
        BlockerId::new(UUID_A).unwrap(),
        attempt_id.clone(),
        "late",
        UnixMillis::new(21),
    )
    .unwrap()
    .resolve(UnixMillis::new(22))
    .unwrap();
    assert!(
        AttemptV1::new(AttemptInputV1 {
            attempt_id,
            session_id,
            stage: &stage,
            number: 1,
            lifecycle: AttemptLifecycle::Skipped,
            started_at: UnixMillis::new(10),
            ended_at: Some(UnixMillis::new(20)),
            reason: None,
            item_slots: vec![slot],
            blockers: vec![late_blocker],
        })
        .is_err()
    );
    let terminal_stage = StageSpecV1::new(
        stage_id("terminal"),
        "Terminal",
        Vec::new(),
        Vec::new(),
        SkipPolicyV1::not_allowed(),
    )
    .unwrap();
    let terminal_session_id = SessionId::new(UUID_B).unwrap();
    let after_terminal = plain_attempt(PlainAttemptFixture {
        attempt: UUID_C,
        session_id: &terminal_session_id,
        stage: &terminal_stage,
        number: 1,
        lifecycle: AttemptLifecycle::Completed,
        started_at: 10,
        ended_at: Some(21),
        reason: None,
    });
    assert!(
        SessionAggregateV1::new(SessionAggregateInputV1 {
            session_id: terminal_session_id,
            task_title: "Task".to_owned(),
            snapshot: procedure_snapshot(
                vec![terminal_stage.clone()],
                vec![
                    ProcedureWarningCodeV1::StageHasNoRequiredItems,
                    ProcedureWarningCodeV1::AnyPreviousReturnPolicy,
                ],
            ),
            lifecycle: SessionLifecycle::Completed,
            revision: Revision::new(2),
            stage_progress: vec![
                StageProgressV1::new(
                    terminal_stage.id().clone(),
                    0,
                    StageProgressState::Done,
                    1,
                    Some(after_terminal.attempt_id().clone()),
                )
                .unwrap(),
            ],
            attempts: vec![after_terminal],
            active_stage_id: None,
            active_attempt_id: None,
            created_at: UnixMillis::new(10),
            completed_at: Some(UnixMillis::new(20)),
            cancelled_at: None,
            cancel_reason: None,
        })
        .is_err()
    );
}

#[test]
fn aggregate_validation_couples_history_frontiers_and_chronology() {
    let first = StageSpecV1::new(
        stage_id("first"),
        "First",
        Vec::new(),
        Vec::new(),
        SkipPolicyV1::allowed(false),
    )
    .unwrap();
    let second = StageSpecV1::new(
        stage_id("second"),
        "Second",
        Vec::new(),
        Vec::new(),
        SkipPolicyV1::allowed(false),
    )
    .unwrap();
    let third = StageSpecV1::new(
        stage_id("third"),
        "Third",
        Vec::new(),
        Vec::new(),
        SkipPolicyV1::allowed(false),
    )
    .unwrap();
    let snapshot = procedure_snapshot(
        vec![first.clone(), second.clone(), third.clone()],
        vec![
            ProcedureWarningCodeV1::StageHasNoRequiredItems,
            ProcedureWarningCodeV1::FinalStageSkippable,
            ProcedureWarningCodeV1::AnyPreviousReturnPolicy,
        ],
    );
    let session_id = SessionId::new(UUID_B).unwrap();
    let done = plain_attempt(PlainAttemptFixture {
        attempt: UUID_A,
        session_id: &session_id,
        stage: &first,
        number: 1,
        lifecycle: AttemptLifecycle::Completed,
        started_at: 10,
        ended_at: Some(11),
        reason: None,
    });
    let active = plain_attempt(PlainAttemptFixture {
        attempt: UUID_B,
        session_id: &session_id,
        stage: &second,
        number: 1,
        lifecycle: AttemptLifecycle::Active,
        started_at: 11,
        ended_at: None,
        reason: None,
    });
    let valid_running_progress = vec![
        StageProgressV1::new(
            first.id().clone(),
            0,
            StageProgressState::Done,
            1,
            Some(done.attempt_id().clone()),
        )
        .unwrap(),
        StageProgressV1::current(second.id().clone(), 1, active.attempt_id().clone(), 1).unwrap(),
        StageProgressV1::pending(third.id().clone(), 2),
    ];
    let hydrated = SessionAggregateV1::new(SessionAggregateInputV1 {
        session_id: session_id.clone(),
        task_title: "Task".to_owned(),
        snapshot: snapshot.clone(),
        lifecycle: SessionLifecycle::Running,
        revision: Revision::new(2),
        stage_progress: valid_running_progress.clone(),
        attempts: vec![done.clone(), active.clone()],
        active_stage_id: Some(second.id().clone()),
        active_attempt_id: Some(active.attempt_id().clone()),
        created_at: UnixMillis::new(10),
        completed_at: None,
        cancelled_at: None,
        cancel_reason: None,
    })
    .unwrap();
    assert_eq!(hydrated.revision(), Revision::new(2));
    assert!(
        SessionAggregateV1::new(SessionAggregateInputV1 {
            session_id: session_id.clone(),
            task_title: "Task".to_owned(),
            snapshot: snapshot.clone(),
            lifecycle: SessionLifecycle::Running,
            revision: Revision::ZERO,
            stage_progress: valid_running_progress.clone(),
            attempts: vec![done.clone(), active.clone()],
            active_stage_id: Some(second.id().clone()),
            active_attempt_id: Some(active.attempt_id().clone()),
            created_at: UnixMillis::new(10),
            completed_at: None,
            cancelled_at: None,
            cancel_reason: None,
        })
        .is_err()
    );
    let mismatched_latest = StageProgressV1::new(
        first.id().clone(),
        0,
        StageProgressState::Skipped,
        1,
        Some(done.attempt_id().clone()),
    )
    .unwrap();
    assert!(
        SessionAggregateV1::new(SessionAggregateInputV1 {
            session_id: session_id.clone(),
            task_title: "Task".to_owned(),
            snapshot: snapshot.clone(),
            lifecycle: SessionLifecycle::Running,
            revision: Revision::new(2),
            stage_progress: vec![
                mismatched_latest,
                valid_running_progress[1].clone(),
                valid_running_progress[2].clone(),
            ],
            attempts: vec![done.clone(), active.clone()],
            active_stage_id: Some(second.id().clone()),
            active_attempt_id: Some(active.attempt_id().clone()),
            created_at: UnixMillis::new(10),
            completed_at: None,
            cancelled_at: None,
            cancel_reason: None,
        })
        .is_err()
    );
    assert!(
        StageProgressV1::new(
            third.id().clone(),
            2,
            StageProgressState::Pending,
            1,
            Some(AttemptId::new(UUID_C).unwrap()),
        )
        .is_err()
    );
    let abandoned_on_return = plain_attempt(PlainAttemptFixture {
        attempt: UUID_B,
        session_id: &session_id,
        stage: &second,
        number: 1,
        lifecycle: AttemptLifecycle::Abandoned,
        started_at: 11,
        ended_at: Some(12),
        reason: Some("return"),
    });
    let revisited = plain_attempt(PlainAttemptFixture {
        attempt: UUID_D,
        session_id: &session_id,
        stage: &first,
        number: 2,
        lifecycle: AttemptLifecycle::Active,
        started_at: 12,
        ended_at: None,
        reason: None,
    });
    assert!(
        SessionAggregateV1::new(SessionAggregateInputV1 {
            session_id: session_id.clone(),
            task_title: "Task".to_owned(),
            snapshot: snapshot.clone(),
            lifecycle: SessionLifecycle::Running,
            revision: Revision::new(3),
            stage_progress: vec![
                StageProgressV1::current(first.id().clone(), 0, revisited.attempt_id().clone(), 2,)
                    .unwrap(),
                StageProgressV1::new(
                    second.id().clone(),
                    1,
                    StageProgressState::Redo,
                    1,
                    Some(abandoned_on_return.attempt_id().clone()),
                )
                .unwrap(),
                StageProgressV1::pending(third.id().clone(), 2),
            ],
            attempts: vec![done.clone(), abandoned_on_return.clone(), revisited.clone(),],
            active_stage_id: Some(first.id().clone()),
            active_attempt_id: Some(revisited.attempt_id().clone()),
            created_at: UnixMillis::new(10),
            completed_at: None,
            cancelled_at: None,
            cancel_reason: None,
        })
        .is_ok()
    );
    assert!(
        SessionAggregateV1::new(SessionAggregateInputV1 {
            session_id: session_id.clone(),
            task_title: "Task".to_owned(),
            snapshot: snapshot.clone(),
            lifecycle: SessionLifecycle::Running,
            revision: Revision::new(2),
            stage_progress: vec![
                StageProgressV1::current(first.id().clone(), 0, active.attempt_id().clone(), 1)
                    .unwrap(),
                StageProgressV1::new(
                    second.id().clone(),
                    1,
                    StageProgressState::Done,
                    1,
                    Some(done.attempt_id().clone()),
                )
                .unwrap(),
                StageProgressV1::pending(third.id().clone(), 2),
            ],
            attempts: vec![
                plain_attempt(PlainAttemptFixture {
                    attempt: UUID_B,
                    session_id: &session_id,
                    stage: &first,
                    number: 1,
                    lifecycle: AttemptLifecycle::Active,
                    started_at: 10,
                    ended_at: None,
                    reason: None,
                }),
                plain_attempt(PlainAttemptFixture {
                    attempt: UUID_A,
                    session_id: &session_id,
                    stage: &second,
                    number: 1,
                    lifecycle: AttemptLifecycle::Completed,
                    started_at: 11,
                    ended_at: Some(12),
                    reason: None,
                }),
            ],
            active_stage_id: Some(first.id().clone()),
            active_attempt_id: Some(active.attempt_id().clone()),
            created_at: UnixMillis::new(10),
            completed_at: None,
            cancelled_at: None,
            cancel_reason: None,
        })
        .is_err()
    );
    let abandoned = plain_attempt(PlainAttemptFixture {
        attempt: UUID_B,
        session_id: &session_id,
        stage: &second,
        number: 1,
        lifecycle: AttemptLifecycle::Abandoned,
        started_at: 12,
        ended_at: Some(13),
        reason: Some("cancelled"),
    });
    let skipped = plain_attempt(PlainAttemptFixture {
        attempt: UUID_C,
        session_id: &session_id,
        stage: &third,
        number: 1,
        lifecycle: AttemptLifecycle::Skipped,
        started_at: 14,
        ended_at: Some(20),
        reason: None,
    });
    let cancelled_attempt = plain_attempt(PlainAttemptFixture {
        attempt: UUID_D,
        session_id: &session_id,
        stage: &second,
        number: 1,
        lifecycle: AttemptLifecycle::Abandoned,
        started_at: 11,
        ended_at: Some(20),
        reason: Some("cancelled"),
    });
    assert!(
        SessionAggregateV1::new(SessionAggregateInputV1 {
            session_id: session_id.clone(),
            task_title: "Task".to_owned(),
            snapshot: snapshot.clone(),
            lifecycle: SessionLifecycle::Cancelled,
            revision: Revision::new(3),
            stage_progress: vec![
                StageProgressV1::new(
                    first.id().clone(),
                    0,
                    StageProgressState::Done,
                    1,
                    Some(done.attempt_id().clone()),
                )
                .unwrap(),
                StageProgressV1::new(
                    second.id().clone(),
                    1,
                    StageProgressState::Abandoned,
                    1,
                    Some(cancelled_attempt.attempt_id().clone()),
                )
                .unwrap(),
                StageProgressV1::pending(third.id().clone(), 2),
            ],
            attempts: vec![done.clone(), cancelled_attempt.clone()],
            active_stage_id: None,
            active_attempt_id: None,
            created_at: UnixMillis::new(10),
            completed_at: None,
            cancelled_at: Some(UnixMillis::new(20)),
            cancel_reason: Some("cancelled".to_owned()),
        })
        .is_ok()
    );
    assert!(
        SessionAggregateV1::new(SessionAggregateInputV1 {
            session_id: session_id.clone(),
            task_title: "Task".to_owned(),
            snapshot: snapshot.clone(),
            lifecycle: SessionLifecycle::Cancelled,
            revision: Revision::new(3),
            stage_progress: vec![
                StageProgressV1::new(
                    first.id().clone(),
                    0,
                    StageProgressState::Done,
                    1,
                    Some(done.attempt_id().clone()),
                )
                .unwrap(),
                StageProgressV1::new(
                    second.id().clone(),
                    1,
                    StageProgressState::Abandoned,
                    1,
                    Some(cancelled_attempt.attempt_id().clone()),
                )
                .unwrap(),
                StageProgressV1::pending(third.id().clone(), 2),
            ],
            attempts: vec![done.clone(), cancelled_attempt.clone()],
            active_stage_id: None,
            active_attempt_id: None,
            created_at: UnixMillis::new(10),
            completed_at: None,
            cancelled_at: Some(UnixMillis::new(20)),
            cancel_reason: Some("different cancellation reason".to_owned()),
        })
        .is_err()
    );
    let completed_second = plain_attempt(PlainAttemptFixture {
        attempt: UUID_B,
        session_id: &session_id,
        stage: &second,
        number: 1,
        lifecycle: AttemptLifecycle::Completed,
        started_at: 12,
        ended_at: Some(13),
        reason: None,
    });
    let late_abandoned = plain_attempt(PlainAttemptFixture {
        attempt: UUID_C,
        session_id: &session_id,
        stage: &third,
        number: 1,
        lifecycle: AttemptLifecycle::Abandoned,
        started_at: 14,
        ended_at: Some(15),
        reason: Some("late"),
    });
    assert!(
        SessionAggregateV1::new(SessionAggregateInputV1 {
            session_id: session_id.clone(),
            task_title: "Task".to_owned(),
            snapshot: snapshot.clone(),
            lifecycle: SessionLifecycle::Cancelled,
            revision: Revision::new(4),
            stage_progress: vec![
                StageProgressV1::new(
                    first.id().clone(),
                    0,
                    StageProgressState::Done,
                    1,
                    Some(done.attempt_id().clone()),
                )
                .unwrap(),
                StageProgressV1::new(
                    second.id().clone(),
                    1,
                    StageProgressState::Done,
                    1,
                    Some(completed_second.attempt_id().clone()),
                )
                .unwrap(),
                StageProgressV1::new(
                    third.id().clone(),
                    2,
                    StageProgressState::Abandoned,
                    1,
                    Some(late_abandoned.attempt_id().clone()),
                )
                .unwrap(),
            ],
            attempts: vec![done.clone(), completed_second, late_abandoned],
            active_stage_id: None,
            active_attempt_id: None,
            created_at: UnixMillis::new(10),
            completed_at: None,
            cancelled_at: Some(UnixMillis::new(20)),
            cancel_reason: Some("cancelled".to_owned()),
        })
        .is_err()
    );
    assert!(
        SessionAggregateV1::new(SessionAggregateInputV1 {
            session_id: session_id.clone(),
            task_title: "Task".to_owned(),
            snapshot: snapshot.clone(),
            lifecycle: SessionLifecycle::Completed,
            revision: Revision::new(4),
            stage_progress: vec![
                StageProgressV1::new(
                    first.id().clone(),
                    0,
                    StageProgressState::Done,
                    1,
                    Some(done.attempt_id().clone()),
                )
                .unwrap(),
                StageProgressV1::new(
                    second.id().clone(),
                    1,
                    StageProgressState::Abandoned,
                    1,
                    Some(abandoned.attempt_id().clone()),
                )
                .unwrap(),
                StageProgressV1::new(
                    third.id().clone(),
                    2,
                    StageProgressState::Skipped,
                    1,
                    Some(skipped.attempt_id().clone()),
                )
                .unwrap(),
            ],
            attempts: vec![done.clone(), abandoned.clone(), skipped.clone()],
            active_stage_id: None,
            active_attempt_id: None,
            created_at: UnixMillis::new(10),
            completed_at: Some(UnixMillis::new(20)),
            cancelled_at: None,
            cancel_reason: None,
        })
        .is_err()
    );
    let pre_session = plain_attempt(PlainAttemptFixture {
        attempt: UUID_A,
        session_id: &session_id,
        stage: &first,
        number: 1,
        lifecycle: AttemptLifecycle::Completed,
        started_at: 9,
        ended_at: Some(10),
        reason: None,
    });
    assert!(
        SessionAggregateV1::new(SessionAggregateInputV1 {
            session_id,
            task_title: "Task".to_owned(),
            snapshot: procedure_snapshot(
                vec![first.clone()],
                vec![
                    ProcedureWarningCodeV1::StageHasNoRequiredItems,
                    ProcedureWarningCodeV1::FinalStageSkippable,
                    ProcedureWarningCodeV1::AnyPreviousReturnPolicy,
                ],
            ),
            lifecycle: SessionLifecycle::Completed,
            revision: Revision::new(2),
            stage_progress: vec![
                StageProgressV1::new(
                    first.id().clone(),
                    0,
                    StageProgressState::Done,
                    1,
                    Some(pre_session.attempt_id().clone()),
                )
                .unwrap(),
            ],
            attempts: vec![pre_session],
            active_stage_id: None,
            active_attempt_id: None,
            created_at: UnixMillis::new(10),
            completed_at: Some(UnixMillis::new(20)),
            cancelled_at: None,
            cancel_reason: None,
        })
        .is_err()
    );
}
#[test]
fn aggregate_history_rejects_overlaps_and_unrecorded_revisits() {
    let first = StageSpecV1::new(
        stage_id("first"),
        "First",
        Vec::new(),
        Vec::new(),
        SkipPolicyV1::not_allowed(),
    )
    .unwrap();
    let second = StageSpecV1::new(
        stage_id("second"),
        "Second",
        Vec::new(),
        Vec::new(),
        SkipPolicyV1::not_allowed(),
    )
    .unwrap();
    let snapshot = procedure_snapshot(
        vec![first.clone(), second.clone()],
        vec![
            ProcedureWarningCodeV1::StageHasNoRequiredItems,
            ProcedureWarningCodeV1::AnyPreviousReturnPolicy,
        ],
    );
    let session_id = SessionId::new(UUID_B).unwrap();
    let first_done = plain_attempt(PlainAttemptFixture {
        attempt: UUID_A,
        session_id: &session_id,
        stage: &first,
        number: 1,
        lifecycle: AttemptLifecycle::Completed,
        started_at: 10,
        ended_at: Some(12),
        reason: None,
    });
    let overlapping_active = plain_attempt(PlainAttemptFixture {
        attempt: UUID_C,
        session_id: &session_id,
        stage: &second,
        number: 1,
        lifecycle: AttemptLifecycle::Active,
        started_at: 11,
        ended_at: None,
        reason: None,
    });
    assert!(
        SessionAggregateV1::new(SessionAggregateInputV1 {
            session_id: session_id.clone(),
            task_title: "Task".to_owned(),
            snapshot: snapshot.clone(),
            lifecycle: SessionLifecycle::Running,
            revision: Revision::new(2),
            stage_progress: vec![
                StageProgressV1::new(
                    first.id().clone(),
                    0,
                    StageProgressState::Done,
                    1,
                    Some(first_done.attempt_id().clone()),
                )
                .unwrap(),
                StageProgressV1::current(
                    second.id().clone(),
                    1,
                    overlapping_active.attempt_id().clone(),
                    1,
                )
                .unwrap(),
            ],
            attempts: vec![first_done, overlapping_active.clone()],
            active_stage_id: Some(second.id().clone()),
            active_attempt_id: Some(overlapping_active.attempt_id().clone()),
            created_at: UnixMillis::new(10),
            completed_at: None,
            cancelled_at: None,
            cancel_reason: None,
        })
        .is_err()
    );

    let unrecorded_revisit = plain_attempt(PlainAttemptFixture {
        attempt: UUID_D,
        session_id: &session_id,
        stage: &first,
        number: 2,
        lifecycle: AttemptLifecycle::Active,
        started_at: 10,
        ended_at: None,
        reason: None,
    });
    assert!(
        SessionAggregateV1::new(SessionAggregateInputV1 {
            session_id,
            task_title: "Task".to_owned(),
            snapshot,
            lifecycle: SessionLifecycle::Running,
            revision: Revision::new(1),
            stage_progress: vec![
                StageProgressV1::current(
                    first.id().clone(),
                    0,
                    unrecorded_revisit.attempt_id().clone(),
                    2,
                )
                .unwrap(),
                StageProgressV1::pending(second.id().clone(), 1),
            ],
            attempts: vec![unrecorded_revisit.clone()],
            active_stage_id: Some(first.id().clone()),
            active_attempt_id: Some(unrecorded_revisit.attempt_id().clone()),
            created_at: UnixMillis::new(10),
            completed_at: None,
            cancelled_at: None,
            cancel_reason: None,
        })
        .is_err()
    );
}
#[test]
fn hydrated_retry_history_rejects_reused_ids_and_noncontiguous_numbers() {
    let stage = StageSpecV1::new(
        stage_id("retry"),
        "Retry",
        Vec::new(),
        Vec::new(),
        SkipPolicyV1::not_allowed(),
    )
    .unwrap();
    let snapshot = procedure_snapshot(
        vec![stage.clone()],
        vec![
            ProcedureWarningCodeV1::StageHasNoRequiredItems,
            ProcedureWarningCodeV1::AnyPreviousReturnPolicy,
        ],
    );
    let session_id = SessionId::new(UUID_B).unwrap();
    let abandoned = hydrated_attempt(
        UUID_A,
        &session_id,
        &stage,
        1,
        AttemptLifecycle::Abandoned,
        10,
        Some(20),
        Some("retry"),
        Vec::new(),
    )
    .unwrap();
    let retry = hydrated_attempt(
        UUID_C,
        &session_id,
        &stage,
        2,
        AttemptLifecycle::Active,
        20,
        None,
        None,
        Vec::new(),
    )
    .unwrap();
    let retry_progress =
        StageProgressV1::current(stage.id().clone(), 0, retry.attempt_id().clone(), 2).unwrap();
    let aggregate = hydrated_aggregate(
        session_id.clone(),
        snapshot.clone(),
        SessionLifecycle::Running,
        2,
        vec![retry_progress],
        vec![abandoned.clone(), retry.clone()],
        Some(stage.id().clone()),
        Some(retry.attempt_id().clone()),
        10,
        None,
        None,
        None,
    )
    .unwrap();
    assert_eq!(aggregate.latest_recorded_at(), UnixMillis::new(20));

    let non_contiguous = hydrated_attempt(
        UUID_D,
        &session_id,
        &stage,
        3,
        AttemptLifecycle::Active,
        20,
        None,
        None,
        Vec::new(),
    )
    .unwrap();
    assert_invalid_state(
        hydrated_aggregate(
            session_id.clone(),
            snapshot.clone(),
            SessionLifecycle::Running,
            2,
            vec![
                StageProgressV1::current(
                    stage.id().clone(),
                    0,
                    non_contiguous.attempt_id().clone(),
                    3,
                )
                .unwrap(),
            ],
            vec![abandoned.clone(), non_contiguous.clone()],
            Some(stage.id().clone()),
            Some(non_contiguous.attempt_id().clone()),
            10,
            None,
            None,
            None,
        ),
        "attempt numbers must increase by one within their stage",
    );

    let reused_id = hydrated_attempt(
        UUID_A,
        &session_id,
        &stage,
        2,
        AttemptLifecycle::Active,
        20,
        None,
        None,
        Vec::new(),
    )
    .unwrap();
    assert_invalid_state(
        hydrated_aggregate(
            session_id.clone(),
            snapshot.clone(),
            SessionLifecycle::Running,
            2,
            vec![
                StageProgressV1::current(stage.id().clone(), 0, reused_id.attempt_id().clone(), 2)
                    .unwrap(),
            ],
            vec![abandoned.clone(), reused_id.clone()],
            Some(stage.id().clone()),
            Some(reused_id.attempt_id().clone()),
            10,
            None,
            None,
            None,
        ),
        "session attempt identifiers must be unique",
    );
}
#[test]
fn hydrated_return_history_canonicalizes_attempts_and_rejects_inconsistent_progress_frontiers() {
    let first = StageSpecV1::new(
        stage_id("return-first"),
        "First",
        Vec::new(),
        Vec::new(),
        SkipPolicyV1::not_allowed(),
    )
    .unwrap();
    let second = StageSpecV1::new(
        stage_id("return-second"),
        "Second",
        Vec::new(),
        Vec::new(),
        SkipPolicyV1::not_allowed(),
    )
    .unwrap();
    let third = StageSpecV1::new(
        stage_id("return-third"),
        "Third",
        Vec::new(),
        Vec::new(),
        SkipPolicyV1::not_allowed(),
    )
    .unwrap();
    let snapshot = procedure_snapshot(
        vec![first.clone(), second.clone(), third.clone()],
        vec![
            ProcedureWarningCodeV1::StageHasNoRequiredItems,
            ProcedureWarningCodeV1::AnyPreviousReturnPolicy,
        ],
    );
    let session_id = SessionId::new(UUID_B).unwrap();
    let first_done = hydrated_attempt(
        UUID_A,
        &session_id,
        &first,
        1,
        AttemptLifecycle::Completed,
        10,
        Some(20),
        None,
        Vec::new(),
    )
    .unwrap();
    let returned_from_second = hydrated_attempt(
        UUID_C,
        &session_id,
        &second,
        1,
        AttemptLifecycle::Abandoned,
        20,
        Some(30),
        Some("return to the prior stage"),
        Vec::new(),
    )
    .unwrap();
    let returned_to_first = hydrated_attempt(
        UUID_D,
        &session_id,
        &first,
        2,
        AttemptLifecycle::Active,
        30,
        None,
        None,
        Vec::new(),
    )
    .unwrap();
    let return_progress = vec![
        StageProgressV1::current(
            first.id().clone(),
            0,
            returned_to_first.attempt_id().clone(),
            2,
        )
        .unwrap(),
        StageProgressV1::new(
            second.id().clone(),
            1,
            StageProgressState::Redo,
            1,
            Some(returned_from_second.attempt_id().clone()),
        )
        .unwrap(),
        StageProgressV1::pending(third.id().clone(), 2),
    ];
    let aggregate = hydrated_aggregate(
        session_id.clone(),
        snapshot.clone(),
        SessionLifecycle::Running,
        3,
        return_progress,
        vec![
            first_done.clone(),
            returned_from_second.clone(),
            returned_to_first.clone(),
        ],
        Some(first.id().clone()),
        Some(returned_to_first.attempt_id().clone()),
        10,
        None,
        None,
        None,
    )
    .unwrap();
    assert_eq!(
        aggregate
            .attempts()
            .iter()
            .map(|attempt| (attempt.stage_id().clone(), attempt.number()))
            .collect::<Vec<_>>(),
        vec![
            (first.id().clone(), 1),
            (first.id().clone(), 2),
            (second.id().clone(), 1),
        ]
    );

    let wrong_stage = hydrated_attempt(
        UUID_D,
        &session_id,
        &third,
        1,
        AttemptLifecycle::Active,
        30,
        None,
        None,
        Vec::new(),
    )
    .unwrap();
    assert_invalid_state(
        hydrated_aggregate(
            session_id.clone(),
            snapshot.clone(),
            SessionLifecycle::Running,
            3,
            vec![
                StageProgressV1::new(
                    first.id().clone(),
                    0,
                    StageProgressState::Done,
                    1,
                    Some(first_done.attempt_id().clone()),
                )
                .unwrap(),
                StageProgressV1::new(
                    second.id().clone(),
                    1,
                    StageProgressState::Redo,
                    1,
                    Some(returned_from_second.attempt_id().clone()),
                )
                .unwrap(),
                StageProgressV1::current(
                    third.id().clone(),
                    2,
                    wrong_stage.attempt_id().clone(),
                    1,
                )
                .unwrap(),
            ],
            vec![
                first_done.clone(),
                returned_from_second.clone(),
                wrong_stage.clone(),
            ],
            Some(third.id().clone()),
            Some(wrong_stage.attempt_id().clone()),
            10,
            None,
            None,
            None,
        ),
        "ordinary stage advancement requires a completed or skipped predecessor",
    );

    let completed_second = hydrated_attempt(
        UUID_C,
        &session_id,
        &second,
        1,
        AttemptLifecycle::Completed,
        20,
        Some(30),
        None,
        Vec::new(),
    )
    .unwrap();
    assert_invalid_state(
        hydrated_aggregate(
            session_id,
            snapshot,
            SessionLifecycle::Running,
            3,
            vec![
                StageProgressV1::current(
                    first.id().clone(),
                    0,
                    returned_to_first.attempt_id().clone(),
                    2,
                )
                .unwrap(),
                StageProgressV1::new(
                    second.id().clone(),
                    1,
                    StageProgressState::Done,
                    1,
                    Some(completed_second.attempt_id().clone()),
                )
                .unwrap(),
                StageProgressV1::pending(third.id().clone(), 2),
            ],
            vec![first_done, completed_second, returned_to_first.clone()],
            Some(first.id().clone()),
            Some(returned_to_first.attempt_id().clone()),
            10,
            None,
            None,
            None,
        ),
        "returns must follow an abandoned reason-bearing attempt",
    );
}
#[test]
fn hydrated_reopen_and_cancel_history_preserves_single_attempt_reason_and_clock_floor() {
    let stage = StageSpecV1::new(
        stage_id("reopen"),
        "Reopen",
        Vec::new(),
        Vec::new(),
        SkipPolicyV1::not_allowed(),
    )
    .unwrap();
    let snapshot = procedure_snapshot(
        vec![stage.clone()],
        vec![
            ProcedureWarningCodeV1::StageHasNoRequiredItems,
            ProcedureWarningCodeV1::AnyPreviousReturnPolicy,
        ],
    );
    let session_id = SessionId::new(UUID_B).unwrap();
    let completed = hydrated_attempt(
        UUID_A,
        &session_id,
        &stage,
        1,
        AttemptLifecycle::Completed,
        10,
        Some(20),
        None,
        Vec::new(),
    )
    .unwrap();
    let reopened = hydrated_attempt(
        UUID_C,
        &session_id,
        &stage,
        2,
        AttemptLifecycle::Active,
        25,
        None,
        Some("new evidence required a reopen"),
        Vec::new(),
    )
    .unwrap();
    let reopened_progress =
        StageProgressV1::current(stage.id().clone(), 0, reopened.attempt_id().clone(), 2).unwrap();
    let reopened_aggregate = hydrated_aggregate(
        session_id.clone(),
        snapshot.clone(),
        SessionLifecycle::Running,
        3,
        vec![reopened_progress],
        vec![completed.clone(), reopened.clone()],
        Some(stage.id().clone()),
        Some(reopened.attempt_id().clone()),
        10,
        None,
        None,
        None,
    )
    .unwrap();
    assert_eq!(
        attempt_by_key(
            &reopened_aggregate,
            stage.id(),
            reopened.attempt_id(),
            reopened.number(),
        )
        .reason(),
        Some("new evidence required a reopen")
    );
    assert_eq!(reopened_aggregate.latest_recorded_at(), UnixMillis::new(25));

    let cancelled = hydrated_attempt(
        UUID_C,
        &session_id,
        &stage,
        2,
        AttemptLifecycle::Abandoned,
        25,
        Some(30),
        Some("terminal cancellation"),
        Vec::new(),
    )
    .unwrap();
    let progress = StageProgressV1::new(
        stage.id().clone(),
        0,
        StageProgressState::Abandoned,
        2,
        Some(cancelled.attempt_id().clone()),
    )
    .unwrap();
    let aggregate = hydrated_aggregate(
        session_id.clone(),
        snapshot.clone(),
        SessionLifecycle::Cancelled,
        4,
        vec![progress.clone()],
        vec![completed.clone(), cancelled.clone()],
        None,
        None,
        10,
        None,
        Some(30),
        Some("terminal cancellation"),
    )
    .unwrap();
    assert_eq!(aggregate.revision(), Revision::new(4));
    assert_eq!(
        attempt_by_key(
            &aggregate,
            stage.id(),
            cancelled.attempt_id(),
            cancelled.number()
        )
        .reason(),
        Some("terminal cancellation")
    );
    assert_eq!(aggregate.latest_recorded_at(), UnixMillis::new(30));

    assert_invalid_state(
        hydrated_attempt(
            UUID_C,
            &session_id,
            &stage,
            2,
            AttemptLifecycle::Abandoned,
            25,
            Some(30),
            None,
            Vec::new(),
        ),
        "attempt lifecycle metadata is inconsistent",
    );
    assert_invalid_state(
        hydrated_aggregate(
            session_id,
            snapshot,
            SessionLifecycle::Cancelled,
            4,
            vec![progress],
            vec![completed, cancelled],
            None,
            None,
            10,
            None,
            Some(30),
            Some("different cancellation reason"),
        ),
        "cancelled session does not align with its abandoned attempt",
    );
}
#[test]
fn terminal_session_attempt_must_be_chronologically_last_at_equal_milliseconds() {
    let first = StageSpecV1::new(
        stage_id("first"),
        "First",
        Vec::new(),
        Vec::new(),
        SkipPolicyV1::not_allowed(),
    )
    .unwrap();
    let final_stage = StageSpecV1::new(
        stage_id("final"),
        "Final",
        Vec::new(),
        Vec::new(),
        SkipPolicyV1::not_allowed(),
    )
    .unwrap();
    let warnings = vec![
        ProcedureWarningCodeV1::StageHasNoRequiredItems,
        ProcedureWarningCodeV1::AnyPreviousReturnPolicy,
    ];

    let completed_snapshot =
        procedure_snapshot(vec![first.clone(), final_stage.clone()], warnings.clone());
    let completed_session_id = SessionId::new(UUID_D).unwrap();
    let completed_first = plain_attempt(PlainAttemptFixture {
        attempt: UUID_A,
        session_id: &completed_session_id,
        stage: &first,
        number: 1,
        lifecycle: AttemptLifecycle::Completed,
        started_at: 10,
        ended_at: Some(15),
        reason: None,
    });
    let completed_terminal = plain_attempt(PlainAttemptFixture {
        attempt: UUID_B,
        session_id: &completed_session_id,
        stage: &final_stage,
        number: 1,
        lifecycle: AttemptLifecycle::Completed,
        started_at: 15,
        ended_at: Some(20),
        reason: None,
    });
    let completed_after_terminal = plain_attempt(PlainAttemptFixture {
        attempt: UUID_C,
        session_id: &completed_session_id,
        stage: &first,
        number: 2,
        lifecycle: AttemptLifecycle::Completed,
        started_at: 20,
        ended_at: Some(20),
        reason: Some("audit follow-up"),
    });
    let completed_progress = vec![
        StageProgressV1::new(
            first.id().clone(),
            0,
            StageProgressState::Done,
            2,
            Some(completed_after_terminal.attempt_id().clone()),
        )
        .unwrap(),
        StageProgressV1::new(
            final_stage.id().clone(),
            1,
            StageProgressState::Done,
            1,
            Some(completed_terminal.attempt_id().clone()),
        )
        .unwrap(),
    ];
    assert!(
        hydrated_aggregate(
            completed_session_id.clone(),
            completed_snapshot.clone(),
            SessionLifecycle::Completed,
            3,
            vec![
                StageProgressV1::new(
                    first.id().clone(),
                    0,
                    StageProgressState::Done,
                    1,
                    Some(completed_first.attempt_id().clone()),
                )
                .unwrap(),
                StageProgressV1::new(
                    final_stage.id().clone(),
                    1,
                    StageProgressState::Done,
                    1,
                    Some(completed_terminal.attempt_id().clone()),
                )
                .unwrap(),
            ],
            vec![completed_first.clone(), completed_terminal.clone()],
            None,
            None,
            10,
            Some(20),
            None,
            None,
        )
        .is_ok()
    );
    assert_invalid_state(
        hydrated_aggregate(
            completed_session_id,
            completed_snapshot,
            SessionLifecycle::Completed,
            8,
            completed_progress,
            vec![
                completed_first,
                completed_terminal,
                completed_after_terminal,
            ],
            None,
            None,
            10,
            Some(20),
            None,
            None,
        ),
        "completed session terminal attempt is not chronologically last",
    );

    let cancelled_snapshot = procedure_snapshot(vec![first.clone(), final_stage.clone()], warnings);
    let cancelled_session_id = SessionId::new(UUID_D).unwrap();
    let cancelled_first = plain_attempt(PlainAttemptFixture {
        attempt: UUID_A,
        session_id: &cancelled_session_id,
        stage: &first,
        number: 1,
        lifecycle: AttemptLifecycle::Completed,
        started_at: 10,
        ended_at: Some(15),
        reason: None,
    });
    let cancelled_terminal = plain_attempt(PlainAttemptFixture {
        attempt: UUID_B,
        session_id: &cancelled_session_id,
        stage: &final_stage,
        number: 1,
        lifecycle: AttemptLifecycle::Abandoned,
        started_at: 15,
        ended_at: Some(20),
        reason: Some("cancelled"),
    });
    let cancelled_after_terminal = plain_attempt(PlainAttemptFixture {
        attempt: UUID_C,
        session_id: &cancelled_session_id,
        stage: &first,
        number: 2,
        lifecycle: AttemptLifecycle::Completed,
        started_at: 20,
        ended_at: Some(20),
        reason: None,
    });
    let cancelled_progress = vec![
        StageProgressV1::new(
            first.id().clone(),
            0,
            StageProgressState::Done,
            2,
            Some(cancelled_after_terminal.attempt_id().clone()),
        )
        .unwrap(),
        StageProgressV1::new(
            final_stage.id().clone(),
            1,
            StageProgressState::Abandoned,
            1,
            Some(cancelled_terminal.attempt_id().clone()),
        )
        .unwrap(),
    ];
    assert!(
        hydrated_aggregate(
            cancelled_session_id.clone(),
            cancelled_snapshot.clone(),
            SessionLifecycle::Cancelled,
            3,
            vec![
                StageProgressV1::new(
                    first.id().clone(),
                    0,
                    StageProgressState::Done,
                    1,
                    Some(cancelled_first.attempt_id().clone()),
                )
                .unwrap(),
                StageProgressV1::new(
                    final_stage.id().clone(),
                    1,
                    StageProgressState::Abandoned,
                    1,
                    Some(cancelled_terminal.attempt_id().clone()),
                )
                .unwrap(),
            ],
            vec![cancelled_first.clone(), cancelled_terminal.clone()],
            None,
            None,
            10,
            None,
            Some(20),
            Some("cancelled"),
        )
        .is_ok()
    );
    assert_invalid_state(
        hydrated_aggregate(
            cancelled_session_id,
            cancelled_snapshot,
            SessionLifecycle::Cancelled,
            8,
            cancelled_progress,
            vec![
                cancelled_first,
                cancelled_terminal,
                cancelled_after_terminal,
            ],
            None,
            None,
            10,
            None,
            Some(20),
            Some("cancelled"),
        ),
        "cancelled session terminal attempt is not chronologically last",
    );
}
#[test]
fn hydrated_reason_bearing_active_attempts_require_reopen_frontier_facts() {
    let first = StageSpecV1::new(
        stage_id("reopen-first"),
        "First",
        Vec::new(),
        Vec::new(),
        SkipPolicyV1::not_allowed(),
    )
    .unwrap();
    let second = StageSpecV1::new(
        stage_id("reopen-second"),
        "Second",
        Vec::new(),
        Vec::new(),
        SkipPolicyV1::not_allowed(),
    )
    .unwrap();
    let third = StageSpecV1::new(
        stage_id("reopen-third"),
        "Third",
        Vec::new(),
        Vec::new(),
        SkipPolicyV1::not_allowed(),
    )
    .unwrap();
    let snapshot = procedure_snapshot(
        vec![first.clone(), second.clone(), third.clone()],
        vec![
            ProcedureWarningCodeV1::StageHasNoRequiredItems,
            ProcedureWarningCodeV1::AnyPreviousReturnPolicy,
        ],
    );
    let session_id = SessionId::new(UUID_B).unwrap();
    let first_completed = hydrated_attempt(
        UUID_A,
        &session_id,
        &first,
        1,
        AttemptLifecycle::Completed,
        10,
        Some(20),
        None,
        Vec::new(),
    )
    .unwrap();
    let second_completed = hydrated_attempt(
        UUID_C,
        &session_id,
        &second,
        1,
        AttemptLifecycle::Completed,
        20,
        Some(30),
        None,
        Vec::new(),
    )
    .unwrap();
    let third_completed = hydrated_attempt(
        UUID_D,
        &session_id,
        &third,
        1,
        AttemptLifecycle::Completed,
        30,
        Some(40),
        None,
        Vec::new(),
    )
    .unwrap();
    let reopened = hydrated_attempt(
        UUID_B,
        &session_id,
        &first,
        2,
        AttemptLifecycle::Active,
        40,
        None,
        Some("follow-up"),
        Vec::new(),
    )
    .unwrap();
    let reopen_progress = vec![
        StageProgressV1::current(
            first.id().clone(),
            0,
            reopened.attempt_id().clone(),
            reopened.number(),
        )
        .unwrap(),
        StageProgressV1::new(
            second.id().clone(),
            1,
            StageProgressState::Redo,
            second_completed.number(),
            Some(second_completed.attempt_id().clone()),
        )
        .unwrap(),
        StageProgressV1::new(
            third.id().clone(),
            2,
            StageProgressState::Redo,
            third_completed.number(),
            Some(third_completed.attempt_id().clone()),
        )
        .unwrap(),
    ];
    assert!(
        hydrated_aggregate(
            session_id.clone(),
            snapshot.clone(),
            SessionLifecycle::Running,
            5,
            reopen_progress.clone(),
            vec![
                first_completed.clone(),
                second_completed.clone(),
                third_completed.clone(),
                reopened.clone(),
            ],
            Some(first.id().clone()),
            Some(reopened.attempt_id().clone()),
            10,
            None,
            None,
            None,
        )
        .is_ok()
    );

    assert_invalid_state(
        hydrated_aggregate(
            session_id.clone(),
            snapshot.clone(),
            SessionLifecycle::Running,
            5,
            vec![
                reopen_progress[0].clone(),
                reopen_progress[1].clone(),
                StageProgressV1::pending(third.id().clone(), 2),
            ],
            vec![
                first_completed.clone(),
                second_completed.clone(),
                reopened.clone(),
            ],
            Some(first.id().clone()),
            Some(reopened.attempt_id().clone()),
            10,
            None,
            None,
            None,
        ),
        "reason-bearing active or completed attempts must follow a completed final-stage attempt",
    );
    assert_invalid_state(
        hydrated_aggregate(
            session_id.clone(),
            snapshot.clone(),
            SessionLifecycle::Running,
            5,
            vec![
                reopen_progress[0].clone(),
                StageProgressV1::new(
                    second.id().clone(),
                    1,
                    StageProgressState::Done,
                    second_completed.number(),
                    Some(second_completed.attempt_id().clone()),
                )
                .unwrap(),
                StageProgressV1::new(
                    third.id().clone(),
                    2,
                    StageProgressState::Done,
                    third_completed.number(),
                    Some(third_completed.attempt_id().clone()),
                )
                .unwrap(),
            ],
            vec![
                first_completed.clone(),
                second_completed.clone(),
                third_completed.clone(),
                reopened.clone(),
            ],
            Some(first.id().clone()),
            Some(reopened.attempt_id().clone()),
            10,
            None,
            None,
            None,
        ),
        "running session stage progress is inconsistent",
    );

    let restricted_snapshot = ProcedureSnapshotV1::assemble(ProcedureSnapshotAssemblyInputV1 {
        snapshot_id: ProcedureSnapshotId::new(UUID_A).unwrap(),
        procedure_id: "restricted-reopen".to_owned(),
        procedure_version: "1".to_owned(),
        name: "Restricted reopen".to_owned(),
        description: None,
        stages: vec![first.clone(), second.clone(), third.clone()],
        return_policy: ReturnPolicyV1::only(vec![second.id().clone()]).unwrap(),
        source_label: ProcedureSourceLabelV1::new("preset: restricted-reopen").unwrap(),
        accepted_warning_codes: vec![ProcedureWarningCodeV1::StageHasNoRequiredItems],
        created_at: UnixMillis::new(10),
    })
    .unwrap();
    assert_invalid_state(
        hydrated_aggregate(
            session_id.clone(),
            restricted_snapshot.clone(),
            SessionLifecycle::Running,
            5,
            reopen_progress.clone(),
            vec![
                first_completed.clone(),
                second_completed.clone(),
                third_completed.clone(),
                reopened.clone(),
            ],
            Some(first.id().clone()),
            Some(reopened.attempt_id().clone()),
            10,
            None,
            None,
            None,
        ),
        "reason-bearing reopened attempts must target an allowed return destination",
    );
    let returned_from_third = hydrated_attempt(
        UUID_D,
        &session_id,
        &third,
        1,
        AttemptLifecycle::Abandoned,
        30,
        Some(40),
        Some("forged return"),
        Vec::new(),
    )
    .unwrap();
    let forged_return_destination = hydrated_attempt(
        UUID_B,
        &session_id,
        &first,
        2,
        AttemptLifecycle::Active,
        40,
        None,
        None,
        Vec::new(),
    )
    .unwrap();
    assert_invalid_state(
        hydrated_aggregate(
            session_id.clone(),
            restricted_snapshot.clone(),
            SessionLifecycle::Running,
            4,
            vec![
                StageProgressV1::current(
                    first.id().clone(),
                    0,
                    forged_return_destination.attempt_id().clone(),
                    forged_return_destination.number(),
                )
                .unwrap(),
                StageProgressV1::new(
                    second.id().clone(),
                    1,
                    StageProgressState::Redo,
                    second_completed.number(),
                    Some(second_completed.attempt_id().clone()),
                )
                .unwrap(),
                StageProgressV1::new(
                    third.id().clone(),
                    2,
                    StageProgressState::Redo,
                    returned_from_third.number(),
                    Some(returned_from_third.attempt_id().clone()),
                )
                .unwrap(),
            ],
            vec![
                first_completed.clone(),
                second_completed.clone(),
                returned_from_third,
                forged_return_destination.clone(),
            ],
            Some(first.id().clone()),
            Some(forged_return_destination.attempt_id().clone()),
            10,
            None,
            None,
            None,
        ),
        "return destination is not allowed by the immutable procedure",
    );
    let forged_completed_reopen = hydrated_attempt(
        UUID_B,
        &session_id,
        &first,
        2,
        AttemptLifecycle::Completed,
        40,
        Some(50),
        Some("forged completed reopen"),
        Vec::new(),
    )
    .unwrap();
    assert_invalid_state(
        hydrated_aggregate(
            session_id,
            restricted_snapshot,
            SessionLifecycle::Running,
            6,
            vec![
                StageProgressV1::new(
                    first.id().clone(),
                    0,
                    StageProgressState::Done,
                    forged_completed_reopen.number(),
                    Some(forged_completed_reopen.attempt_id().clone()),
                )
                .unwrap(),
                StageProgressV1::new(
                    second.id().clone(),
                    1,
                    StageProgressState::Done,
                    second_completed.number(),
                    Some(second_completed.attempt_id().clone()),
                )
                .unwrap(),
                StageProgressV1::new(
                    third.id().clone(),
                    2,
                    StageProgressState::Done,
                    third_completed.number(),
                    Some(third_completed.attempt_id().clone()),
                )
                .unwrap(),
            ],
            vec![
                first_completed,
                second_completed,
                third_completed,
                forged_completed_reopen,
            ],
            None,
            None,
            10,
            None,
            None,
            None,
        ),
        "reason-bearing reopened attempts must target an allowed return destination",
    );
}
#[test]
fn hydrated_blockers_preserve_timestamps_and_reject_duplicate_or_out_of_lifetime_rows() {
    let stage = StageSpecV1::new(
        stage_id("cancel"),
        "Cancel",
        Vec::new(),
        Vec::new(),
        SkipPolicyV1::not_allowed(),
    )
    .unwrap();
    let snapshot = procedure_snapshot(
        vec![stage.clone()],
        vec![
            ProcedureWarningCodeV1::StageHasNoRequiredItems,
            ProcedureWarningCodeV1::AnyPreviousReturnPolicy,
        ],
    );
    let session_id = SessionId::new(UUID_B).unwrap();
    let attempt_id = AttemptId::new(UUID_C).unwrap();
    let first_blocker = BlockerV1::open(
        BlockerId::new(UUID_A).unwrap(),
        attempt_id.clone(),
        "first blocker",
        UnixMillis::new(12),
    )
    .unwrap()
    .resolve(UnixMillis::new(20))
    .unwrap();
    let second_blocker = BlockerV1::new(BlockerInputV1 {
        blocker_id: BlockerId::new(UUID_B).unwrap(),
        attempt_id: attempt_id.clone(),
        reason: "second blocker".to_owned(),
        state: BlockerState::Resolved,
        created_at: UnixMillis::new(13),
        resolved_at: Some(UnixMillis::new(20)),
    })
    .unwrap();
    let cancelled = hydrated_attempt(
        UUID_C,
        &session_id,
        &stage,
        1,
        AttemptLifecycle::Abandoned,
        10,
        Some(20),
        Some("cancelled"),
        vec![first_blocker.clone(), second_blocker],
    )
    .unwrap();
    let progress = StageProgressV1::new(
        stage.id().clone(),
        0,
        StageProgressState::Abandoned,
        1,
        Some(cancelled.attempt_id().clone()),
    )
    .unwrap();
    let aggregate = hydrated_aggregate(
        session_id.clone(),
        snapshot.clone(),
        SessionLifecycle::Cancelled,
        4,
        vec![progress],
        vec![cancelled.clone()],
        None,
        None,
        10,
        None,
        Some(20),
        Some("cancelled"),
    )
    .unwrap();
    assert_eq!(aggregate.revision(), Revision::new(4));
    let restored = attempt_by_key(
        &aggregate,
        stage.id(),
        cancelled.attempt_id(),
        cancelled.number(),
    );
    let restored_first = restored
        .blockers()
        .iter()
        .find(|blocker| blocker.blocker_id() == first_blocker.blocker_id())
        .unwrap();
    assert_eq!(restored_first.state(), BlockerState::Resolved);
    assert_eq!(restored_first.created_at(), UnixMillis::new(12));
    assert_eq!(restored_first.resolved_at(), Some(UnixMillis::new(20)));

    let duplicate_blocker = BlockerV1::new(BlockerInputV1 {
        blocker_id: BlockerId::new(UUID_A).unwrap(),
        attempt_id: attempt_id.clone(),
        reason: "duplicate blocker id".to_owned(),
        state: BlockerState::Resolved,
        created_at: UnixMillis::new(13),
        resolved_at: Some(UnixMillis::new(20)),
    })
    .unwrap();
    assert_invalid_state(
        hydrated_attempt(
            UUID_C,
            &session_id,
            &stage,
            1,
            AttemptLifecycle::Abandoned,
            10,
            Some(20),
            Some("cancelled"),
            vec![first_blocker.clone(), duplicate_blocker],
        ),
        "attempt blocker identifiers must be unique",
    );

    let late_resolution = BlockerV1::new(BlockerInputV1 {
        blocker_id: BlockerId::new(UUID_B).unwrap(),
        attempt_id,
        reason: "late resolution".to_owned(),
        state: BlockerState::Resolved,
        created_at: UnixMillis::new(12),
        resolved_at: Some(UnixMillis::new(21)),
    })
    .unwrap();
    assert_invalid_state(
        hydrated_attempt(
            UUID_C,
            &session_id,
            &stage,
            1,
            AttemptLifecycle::Abandoned,
            10,
            Some(20),
            Some("cancelled"),
            vec![late_resolution],
        ),
        "blocker timestamps must fall within the attempt lifetime",
    );
    assert!(
        BlockerV1::new(BlockerInputV1 {
            blocker_id: BlockerId::new(UUID_B).unwrap(),
            attempt_id: AttemptId::new(UUID_C).unwrap(),
            reason: "reversed timestamps".to_owned(),
            state: BlockerState::Resolved,
            created_at: UnixMillis::new(20),
            resolved_at: Some(UnixMillis::new(19)),
        })
        .is_err()
    );
}
#[test]
fn hydrated_item_revisions_are_per_slot_without_synthetic_history_intervals() {
    let first = ItemSpecV1::text(common("first", false), 0, 32, true).unwrap();
    let second = ItemSpecV1::text(common("second", false), 0, 32, true).unwrap();
    let stage = StageSpecV1::new(
        stage_id("item-revisions"),
        "Item revisions",
        Vec::new(),
        vec![first.clone(), second.clone()],
        SkipPolicyV1::not_allowed(),
    )
    .unwrap();
    let snapshot = procedure_snapshot(
        vec![stage.clone()],
        vec![
            ProcedureWarningCodeV1::StageHasNoRequiredItems,
            ProcedureWarningCodeV1::AnyPreviousReturnPolicy,
        ],
    );
    let session_id = SessionId::new(UUID_B).unwrap();
    let attempt_id = AttemptId::new(UUID_C).unwrap();
    let active_attempt = hydrated_active_attempt(
        &attempt_id,
        &session_id,
        &stage,
        vec![
            ItemSlotV1::new(ItemSlotInputV1 {
                attempt_id: attempt_id.clone(),
                specification: &first,
                revision: Revision::new(4),
                value: Some(ItemValueV1::text("first value")),
                created_at: Some(UnixMillis::new(11)),
                updated_at: Some(UnixMillis::new(14)),
            })
            .unwrap(),
            ItemSlotV1::new(ItemSlotInputV1 {
                attempt_id: attempt_id.clone(),
                specification: &second,
                revision: Revision::new(3),
                value: Some(ItemValueV1::text("second value")),
                created_at: Some(UnixMillis::new(12)),
                updated_at: Some(UnixMillis::new(15)),
            })
            .unwrap(),
        ],
    );
    let aggregate = hydrated_aggregate(
        session_id,
        snapshot,
        SessionLifecycle::Running,
        8,
        vec![
            StageProgressV1::current(
                stage.id().clone(),
                0,
                active_attempt.attempt_id().clone(),
                1,
            )
            .unwrap(),
        ],
        vec![active_attempt.clone()],
        Some(stage.id().clone()),
        Some(active_attempt.attempt_id().clone()),
        10,
        None,
        None,
        None,
    )
    .unwrap();
    assert_eq!(aggregate.revision(), Revision::new(8));
    let persisted = attempt_by_key(
        &aggregate,
        stage.id(),
        active_attempt.attempt_id(),
        active_attempt.number(),
    );
    assert_eq!(persisted.item_slots()[0].revision(), Revision::new(4));
    assert_eq!(persisted.item_slots()[1].revision(), Revision::new(3));
    assert_eq!(aggregate.latest_recorded_at(), UnixMillis::new(15));

    assert!(
        ItemSlotV1::new(ItemSlotInputV1 {
            attempt_id,
            specification: &first,
            revision: Revision::ZERO,
            value: Some(ItemValueV1::text("impossible")),
            created_at: Some(UnixMillis::new(11)),
            updated_at: Some(UnixMillis::new(11)),
        })
        .is_err()
    );
}
#[test]
fn aggregate_revision_requires_checked_conservative_retained_mutation_floor() {
    let item = ItemSpecV1::text(common("revision-item", false), 0, 32, true).unwrap();
    let stage = StageSpecV1::new(
        stage_id("revision-floor"),
        "Revision floor",
        Vec::new(),
        vec![item.clone()],
        SkipPolicyV1::not_allowed(),
    )
    .unwrap();
    let snapshot = procedure_snapshot(
        vec![stage.clone()],
        vec![
            ProcedureWarningCodeV1::StageHasNoRequiredItems,
            ProcedureWarningCodeV1::AnyPreviousReturnPolicy,
        ],
    );
    let session_id = SessionId::new(UUID_B).unwrap();
    let attempt_id = AttemptId::new(UUID_C).unwrap();
    let slot = ItemSlotV1::new(ItemSlotInputV1 {
        attempt_id: attempt_id.clone(),
        specification: &item,
        revision: Revision::new(1),
        value: Some(ItemValueV1::text("value")),
        created_at: Some(UnixMillis::new(11)),
        updated_at: Some(UnixMillis::new(11)),
    })
    .unwrap();
    let blocker = BlockerV1::open(
        BlockerId::new(UUID_A).unwrap(),
        attempt_id.clone(),
        "blocked",
        UnixMillis::new(12),
    )
    .unwrap();
    let active = AttemptV1::new(AttemptInputV1 {
        attempt_id: attempt_id.clone(),
        session_id: session_id.clone(),
        stage: &stage,
        number: 1,
        lifecycle: AttemptLifecycle::Active,
        started_at: UnixMillis::new(10),
        ended_at: None,
        reason: None,
        item_slots: vec![slot.clone()],
        blockers: vec![blocker.clone()],
    })
    .unwrap();
    let active_progress =
        StageProgressV1::current(stage.id().clone(), 0, attempt_id.clone(), 1).unwrap();
    let item_only = AttemptV1::new(AttemptInputV1 {
        attempt_id: attempt_id.clone(),
        session_id: session_id.clone(),
        stage: &stage,
        number: 1,
        lifecycle: AttemptLifecycle::Active,
        started_at: UnixMillis::new(10),
        ended_at: None,
        reason: None,
        item_slots: vec![slot],
        blockers: Vec::new(),
    })
    .unwrap();
    assert_invalid_state(
        hydrated_aggregate(
            session_id.clone(),
            snapshot.clone(),
            SessionLifecycle::Running,
            1,
            vec![active_progress.clone()],
            vec![item_only],
            Some(stage.id().clone()),
            Some(attempt_id.clone()),
            10,
            None,
            None,
            None,
        ),
        "session aggregate revision is below the conservative retained-mutation floor",
    );

    let blocker_only = AttemptV1::new(AttemptInputV1 {
        attempt_id: attempt_id.clone(),
        session_id: session_id.clone(),
        stage: &stage,
        number: 1,
        lifecycle: AttemptLifecycle::Active,
        started_at: UnixMillis::new(10),
        ended_at: None,
        reason: None,
        item_slots: vec![ItemSlotV1::new_empty(attempt_id.clone(), &item)],
        blockers: vec![blocker.clone()],
    })
    .unwrap();
    assert_invalid_state(
        hydrated_aggregate(
            session_id.clone(),
            snapshot.clone(),
            SessionLifecycle::Running,
            1,
            vec![active_progress.clone()],
            vec![blocker_only],
            Some(stage.id().clone()),
            Some(attempt_id.clone()),
            10,
            None,
            None,
            None,
        ),
        "session aggregate revision is below the conservative retained-mutation floor",
    );

    assert_invalid_state(
        hydrated_aggregate(
            session_id.clone(),
            snapshot.clone(),
            SessionLifecycle::Running,
            2,
            vec![active_progress.clone()],
            vec![active.clone()],
            Some(stage.id().clone()),
            Some(attempt_id.clone()),
            10,
            None,
            None,
            None,
        ),
        "session aggregate revision is below the conservative retained-mutation floor",
    );
    assert_eq!(
        hydrated_aggregate(
            session_id.clone(),
            snapshot.clone(),
            SessionLifecycle::Running,
            3,
            vec![active_progress],
            vec![active],
            Some(stage.id().clone()),
            Some(attempt_id.clone()),
            10,
            None,
            None,
            None,
        )
        .unwrap()
        .revision(),
        Revision::new(3)
    );

    let completed = hydrated_attempt(
        UUID_C,
        &session_id,
        &stage,
        1,
        AttemptLifecycle::Completed,
        10,
        Some(20),
        None,
        Vec::new(),
    )
    .unwrap();
    let completed_progress = StageProgressV1::new(
        stage.id().clone(),
        0,
        StageProgressState::Done,
        1,
        Some(completed.attempt_id().clone()),
    )
    .unwrap();
    assert_invalid_state(
        hydrated_aggregate(
            session_id.clone(),
            snapshot.clone(),
            SessionLifecycle::Completed,
            1,
            vec![completed_progress.clone()],
            vec![completed.clone()],
            None,
            None,
            10,
            Some(20),
            None,
            None,
        ),
        "session aggregate revision is below the conservative retained-mutation floor",
    );
    assert_eq!(
        hydrated_aggregate(
            session_id.clone(),
            snapshot.clone(),
            SessionLifecycle::Completed,
            2,
            vec![completed_progress],
            vec![completed],
            None,
            None,
            10,
            Some(20),
            None,
            None,
        )
        .unwrap()
        .revision(),
        Revision::new(2)
    );

    let unblocked_before_reopen = BlockerV1::open(
        BlockerId::new(UUID_A).unwrap(),
        AttemptId::new(UUID_C).unwrap(),
        "waiting",
        UnixMillis::new(11),
    )
    .unwrap()
    .resolve(UnixMillis::new(12))
    .unwrap();
    let completed_before_reopen = hydrated_attempt(
        UUID_C,
        &session_id,
        &stage,
        1,
        AttemptLifecycle::Completed,
        10,
        Some(13),
        None,
        vec![unblocked_before_reopen.clone()],
    )
    .unwrap();
    let reopened = hydrated_attempt(
        UUID_D,
        &session_id,
        &stage,
        2,
        AttemptLifecycle::Active,
        14,
        None,
        Some("new evidence"),
        Vec::new(),
    )
    .unwrap();
    let reopened_progress =
        StageProgressV1::current(stage.id().clone(), 0, reopened.attempt_id().clone(), 2).unwrap();
    let reopened_history = vec![completed_before_reopen.clone(), reopened.clone()];
    assert_invalid_state(
        hydrated_aggregate(
            session_id.clone(),
            snapshot.clone(),
            SessionLifecycle::Running,
            4,
            vec![reopened_progress.clone()],
            reopened_history.clone(),
            Some(stage.id().clone()),
            Some(reopened.attempt_id().clone()),
            10,
            None,
            None,
            None,
        ),
        "session aggregate revision is below the conservative retained-mutation floor",
    );
    let hydrated_reopen = hydrated_aggregate(
        session_id.clone(),
        snapshot.clone(),
        SessionLifecycle::Running,
        5,
        vec![reopened_progress],
        reopened_history.clone(),
        Some(stage.id().clone()),
        Some(reopened.attempt_id().clone()),
        10,
        None,
        None,
        None,
    )
    .unwrap();
    assert_eq!(reopened_history[0].blockers()[0], unblocked_before_reopen);
    assert_eq!(
        attempt_by_key(
            &hydrated_reopen,
            stage.id(),
            reopened.attempt_id(),
            reopened.number(),
        )
        .reason(),
        Some("new evidence")
    );
    let overflow_attempt_id = AttemptId::new(UUID_D).unwrap();
    let overflow_slot = ItemSlotV1::new(ItemSlotInputV1 {
        attempt_id: overflow_attempt_id.clone(),
        specification: &item,
        revision: Revision::new(u64::MAX),
        value: Some(ItemValueV1::text("value")),
        created_at: Some(UnixMillis::new(11)),
        updated_at: Some(UnixMillis::new(12)),
    })
    .unwrap();
    let overflow_attempt = hydrated_active_attempt(
        &overflow_attempt_id,
        &session_id,
        &stage,
        vec![overflow_slot],
    );
    assert_eq!(
        hydrated_aggregate(
            session_id,
            snapshot,
            SessionLifecycle::Running,
            u64::MAX,
            vec![
                StageProgressV1::current(stage.id().clone(), 0, overflow_attempt_id.clone(), 1,)
                    .unwrap()
            ],
            vec![overflow_attempt],
            Some(stage.id().clone()),
            Some(overflow_attempt_id),
            10,
            None,
            None,
            None,
        )
        .unwrap_err(),
        DomainError::RevisionOverflow {
            revision: Revision::new(1)
        }
    );
}
#[test]
fn hydrated_blocker_rows_preserve_reason_state_and_timestamps_without_provenance() {
    let stage = StageSpecV1::new(
        stage_id("blocker-rows"),
        "Blocker rows",
        Vec::new(),
        Vec::new(),
        SkipPolicyV1::not_allowed(),
    )
    .unwrap();
    let snapshot = procedure_snapshot(
        vec![stage.clone()],
        vec![
            ProcedureWarningCodeV1::StageHasNoRequiredItems,
            ProcedureWarningCodeV1::AnyPreviousReturnPolicy,
        ],
    );
    let session_id = SessionId::new(UUID_B).unwrap();
    let attempt_id = AttemptId::new(UUID_C).unwrap();

    let explicitly_resolved = BlockerV1::open(
        BlockerId::new(UUID_A).unwrap(),
        attempt_id.clone(),
        "explicitly resolved",
        UnixMillis::new(12),
    )
    .unwrap()
    .resolve(UnixMillis::new(20))
    .unwrap();
    let explicitly_unblocked_attempt = hydrated_attempt(
        UUID_C,
        &session_id,
        &stage,
        1,
        AttemptLifecycle::Abandoned,
        10,
        Some(20),
        Some("cancelled"),
        vec![explicitly_resolved.clone()],
    )
    .unwrap();
    let explicit_history = hydrated_aggregate(
        session_id.clone(),
        snapshot.clone(),
        SessionLifecycle::Cancelled,
        4,
        vec![
            StageProgressV1::new(
                stage.id().clone(),
                0,
                StageProgressState::Abandoned,
                1,
                Some(explicitly_unblocked_attempt.attempt_id().clone()),
            )
            .unwrap(),
        ],
        vec![explicitly_unblocked_attempt.clone()],
        None,
        None,
        10,
        None,
        Some(20),
        Some("cancelled"),
    )
    .unwrap();
    let restored_attempt = attempt_by_key(
        &explicit_history,
        stage.id(),
        explicitly_unblocked_attempt.attempt_id(),
        explicitly_unblocked_attempt.number(),
    );
    let restored_blocker = restored_attempt
        .blockers()
        .iter()
        .find(|blocker| blocker.blocker_id() == explicitly_resolved.blocker_id())
        .unwrap();
    assert_eq!(restored_blocker.reason(), "explicitly resolved");
    assert_eq!(restored_blocker.state(), BlockerState::Resolved);
    assert_eq!(restored_blocker.created_at(), UnixMillis::new(12));
    assert_eq!(restored_blocker.resolved_at(), Some(UnixMillis::new(20)));
    assert_eq!(restored_attempt.ended_at(), Some(UnixMillis::new(20)));

    let unordered_blockers = vec![
        BlockerV1::new(BlockerInputV1 {
            blocker_id: BlockerId::new(UUID_A).unwrap(),
            attempt_id: attempt_id.clone(),
            reason: "later timestamp listed first".to_owned(),
            state: BlockerState::Resolved,
            created_at: UnixMillis::new(13),
            resolved_at: Some(UnixMillis::new(20)),
        })
        .unwrap(),
        BlockerV1::new(BlockerInputV1 {
            blocker_id: BlockerId::new(UUID_B).unwrap(),
            attempt_id,
            reason: "earlier timestamp listed second".to_owned(),
            state: BlockerState::Resolved,
            created_at: UnixMillis::new(12),
            resolved_at: Some(UnixMillis::new(20)),
        })
        .unwrap(),
    ];
    let unordered_attempt = hydrated_attempt(
        UUID_C,
        &session_id,
        &stage,
        1,
        AttemptLifecycle::Abandoned,
        10,
        Some(20),
        Some("cancelled"),
        unordered_blockers,
    )
    .unwrap();
    assert!(
        hydrated_aggregate(
            session_id,
            snapshot,
            SessionLifecycle::Cancelled,
            4,
            vec![
                StageProgressV1::new(
                    stage.id().clone(),
                    0,
                    StageProgressState::Abandoned,
                    1,
                    Some(unordered_attempt.attempt_id().clone()),
                )
                .unwrap()
            ],
            vec![unordered_attempt],
            None,
            None,
            10,
            None,
            Some(20),
            Some("cancelled"),
        )
        .is_ok()
    );
}
#[test]
fn unblock_rejects_cross_attempt_resolution_batches() {
    let stage = StageSpecV1::new(
        stage_id("cross-attempt-unblock"),
        "Cross-attempt unblock",
        Vec::new(),
        Vec::new(),
        SkipPolicyV1::not_allowed(),
    )
    .unwrap();
    let stage_id = stage.id().clone();
    let session = SessionAggregateV1::start(
        SessionId::new(UUID_B).unwrap(),
        "Task",
        procedure_snapshot(
            vec![stage],
            vec![
                ProcedureWarningCodeV1::StageHasNoRequiredItems,
                ProcedureWarningCodeV1::AnyPreviousReturnPolicy,
            ],
        ),
        AttemptId::new(UUID_C).unwrap(),
        UnixMillis::new(10),
    )
    .unwrap();
    let first_attempt_id = session.active_attempt_id().unwrap().clone();
    let historical_blocker_id = BlockerId::new(UUID_A).unwrap();
    let blocked = apply_transition_v1(
        Some(&session),
        &SessionCommandV1::Block(BlockSessionV1 {
            expected_attempt_id: first_attempt_id.clone(),
            blocker_id: historical_blocker_id.clone(),
            reason: "first attempt blocker".to_owned(),
        }),
        CommandContextV1 {
            expected_revision: session.revision(),
            now: UnixMillis::new(11),
        },
    )
    .unwrap()
    .next_aggregate()
    .unwrap()
    .clone();
    let retried = apply_transition_v1(
        Some(&blocked),
        &SessionCommandV1::Retry(RetrySessionV1 {
            expected_attempt_id: first_attempt_id.clone(),
            reason: "retry".to_owned(),
            next_attempt_id: AttemptId::new(UUID_D).unwrap(),
        }),
        CommandContextV1 {
            expected_revision: blocked.revision(),
            now: UnixMillis::new(12),
        },
    )
    .unwrap()
    .next_aggregate()
    .unwrap()
    .clone();
    let current_attempt_id = retried.active_attempt_id().unwrap().clone();
    let current_blocked = apply_transition_v1(
        Some(&retried),
        &SessionCommandV1::Block(BlockSessionV1 {
            expected_attempt_id: current_attempt_id.clone(),
            blocker_id: BlockerId::new(UUID_B).unwrap(),
            reason: "current attempt blocker".to_owned(),
        }),
        CommandContextV1 {
            expected_revision: retried.revision(),
            now: UnixMillis::new(13),
        },
    )
    .unwrap()
    .next_aggregate()
    .unwrap()
    .clone();
    assert_eq!(blocked.revision(), Revision::new(2));
    assert_eq!(retried.revision(), Revision::new(3));
    let terminated = attempt_by_key(&retried, &stage_id, &first_attempt_id, 1);
    assert_eq!(terminated.lifecycle(), AttemptLifecycle::Abandoned);
    assert_eq!(terminated.reason(), Some("retry"));
    assert_eq!(terminated.ended_at(), Some(UnixMillis::new(12)));
    let active = attempt_by_key(&retried, &stage_id, &current_attempt_id, 2);
    assert_eq!(active.lifecycle(), AttemptLifecycle::Active);
    assert_eq!(active.reason(), None);
    assert_eq!(active.started_at(), UnixMillis::new(12));
    assert_eq!(current_blocked.revision(), Revision::new(4));

    let cross_attempt_unblock = SessionCommandV1::Unblock(UnblockSessionV1 {
        expected_attempt_id: current_attempt_id,
        blocker_id: Some(historical_blocker_id),
        unblock_all: false,
    });
    assert_eq!(
        apply_transition_v1(
            Some(&current_blocked),
            &cross_attempt_unblock,
            CommandContextV1 {
                expected_revision: current_blocked.revision(),
                now: UnixMillis::new(14),
            },
        )
        .unwrap_err(),
        DomainError::BlockerNotCurrent
    );
}
