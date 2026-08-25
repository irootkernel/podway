use crate::procedure_v2::invalid;
use crate::{
    ArtifactItemSpecV2, ArtifactValueV1, BoundUnitV2, CheckResultExecutorV2,
    CheckResultInputBasisV2, CheckResultItemSpecV2, CheckResultOutcomeV2, CheckResultValueV2,
    ChoiceItemSpecV2, ConditionStateV2, DomainError, EvidencePredicateV2, GraphNodeId,
    IntegerItemSpecV2, ItemCommonV2, ItemConditionV2, ItemId, ItemPredicateOperatorV2,
    ItemPredicateV2, ItemSpecV2, ItemTypeV1, ListItemSpecV2, OperationId, OptionGuardV2,
    PredicateActualV2, PredicateScalarV2, RecordedItemValueV2, Sha256Digest, TextItemSpecV2,
};

use super::helpers::item;

fn common(id: &str) -> ItemCommonV2 {
    ItemCommonV2::new(
        ItemId::new(id).unwrap(),
        format!("Prompt for {id}"),
        None,
        true,
    )
    .unwrap()
}

#[test]
fn v2grd004_option_guards_are_bounded_and_three_valued() {
    let source = GraphNodeId::new("review").unwrap();
    let item = ItemId::new("findings").unwrap();
    let guard = OptionGuardV2::new(vec![EvidencePredicateV2::new(
        source.clone(),
        item.clone(),
        false,
        ItemPredicateOperatorV2::Equals(PredicateScalarV2::Integer(0)),
    )])
    .unwrap();

    assert_eq!(
        guard.evaluate(|_, _| None).state(),
        ConditionStateV2::Unevaluable
    );
    let zero = RecordedItemValueV2::integer(0);
    assert_eq!(
        guard
            .evaluate(|candidate_source, candidate_item| {
                (candidate_source == &source && candidate_item == &item).then_some(&zero)
            })
            .state(),
        ConditionStateV2::Met
    );
    let one = RecordedItemValueV2::integer(1);
    assert_eq!(
        guard.evaluate(|_, _| Some(&one)).state(),
        ConditionStateV2::Unmet
    );
    assert!(OptionGuardV2::new(Vec::new()).is_err());
    let predicates = (0..4)
        .map(|_| {
            EvidencePredicateV2::new(
                source.clone(),
                item.clone(),
                false,
                ItemPredicateOperatorV2::Equals(PredicateScalarV2::Integer(0)),
            )
        })
        .collect::<Vec<_>>();
    assert!(OptionGuardV2::new(predicates.clone()).is_ok());
    assert!(
        OptionGuardV2::new(
            predicates
                .into_iter()
                .chain([EvidencePredicateV2::new(
                    source,
                    item,
                    false,
                    ItemPredicateOperatorV2::AtMost(0),
                )])
                .collect(),
        )
        .is_err()
    );
}

fn digest(character: char) -> Sha256Digest {
    Sha256Digest::new(format!("sha256:{}", character.to_string().repeat(64))).unwrap()
}

fn check_result(operation_id: &str, outcome: CheckResultOutcomeV2) -> RecordedItemValueV2 {
    RecordedItemValueV2::check_result(
        CheckResultValueV2::new(
            OperationId::new(operation_id).unwrap(),
            digest('a'),
            CheckResultInputBasisV2::new("HEAD and dirty-tree snapshot", digest('b')).unwrap(),
            CheckResultExecutorV2::new("gaori", "1.0.0").unwrap(),
            outcome,
            "The complete development gate passed.",
            digest('c'),
        )
        .unwrap(),
    )
}

#[test]
fn item_specs_enforce_v2_bounds() {
    // v2 item prompt is capped at 300 characters.
    assert!(ItemCommonV2::new(ItemId::new("i").unwrap(), "p".repeat(300), None, true).is_ok());
    assert_eq!(
        ItemCommonV2::new(ItemId::new("i").unwrap(), "p".repeat(301), None, true).unwrap_err(),
        invalid("item prompt")
    );
    // item help is capped at 1000 characters.
    assert!(
        ItemCommonV2::new(ItemId::new("i").unwrap(), "p", Some("h".repeat(1000)), true).is_ok()
    );
    assert_eq!(
        ItemCommonV2::new(ItemId::new("i").unwrap(), "p", Some("h".repeat(1001)), true)
            .unwrap_err(),
        invalid("item help")
    );

    // text max length hard cap is 65_536; one over fails.
    assert!(TextItemSpecV2::new(common("t"), 0, 65_536, true).is_ok());
    assert_eq!(
        TextItemSpecV2::new(common("t"), 0, 65_537, true).unwrap_err(),
        crate::DomainError::BoundExceeded {
            field: "text max_length",
            actual: 65_537,
            maximum: 65_536,
            unit: crate::BoundUnitV2::Scalars,
        }
    );

    // choice count cap is 32; one over fails.
    let choices: Vec<String> = (0..32).map(|i| format!("c-{i}")).collect();
    assert!(ChoiceItemSpecV2::new(common("c"), choices.clone()).is_ok());
    let too_many: Vec<String> = (0..33).map(|i| format!("c-{i}")).collect();
    assert_eq!(
        ChoiceItemSpecV2::new(common("c"), too_many).unwrap_err(),
        invalid("choice count must be between one and 32")
    );

    // each choice value is capped at 120 characters.
    assert!(ChoiceItemSpecV2::new(common("c"), vec!["v".repeat(120)]).is_ok());
    assert_eq!(
        ChoiceItemSpecV2::new(common("c"), vec!["v".repeat(121)]).unwrap_err(),
        invalid("choice")
    );

    // list bounds cap at 1_000 entries of 8_192 scalars with 1_000_000 scalars of total content.
    assert!(ListItemSpecV2::new(common("l"), 0, 1_000, 8_192, 1_000_000, true).is_ok());
    assert_eq!(
        ListItemSpecV2::new(common("l"), 0, 1_001, 500, 1_000_000, true).unwrap_err(),
        crate::DomainError::BoundExceeded {
            field: "list max_items",
            actual: 1_001,
            maximum: 1_000,
            unit: crate::BoundUnitV2::Entries,
        }
    );
    assert_eq!(
        ListItemSpecV2::new(common("l"), 0, 50, 8_193, 1_000_000, true).unwrap_err(),
        crate::DomainError::BoundExceeded {
            field: "list max_item_length",
            actual: 8_193,
            maximum: 8_192,
            unit: crate::BoundUnitV2::Scalars,
        }
    );
    assert_eq!(
        ListItemSpecV2::new(common("l"), 0, 50, 500, 1_000_001, true).unwrap_err(),
        crate::DomainError::BoundExceeded {
            field: "list max_total_length",
            actual: 1_000_001,
            maximum: 1_000_000,
            unit: crate::BoundUnitV2::Scalars,
        }
    );
    assert_eq!(
        ListItemSpecV2::new(common("l"), 0, 50, 500, 0, true).unwrap_err(),
        invalid("invalid list total content constraint")
    );
    // A non-empty entry needs at least one scalar, so min_items may not exceed the total ceiling.
    assert_eq!(
        ListItemSpecV2::new(common("l"), 10, 50, 500, 9, true).unwrap_err(),
        invalid("invalid list total content constraint")
    );
    // The effective ceiling is the tighter of the entry product and the declared total.
    let tight = ListItemSpecV2::new(common("l"), 0, 50, 500, 1_000, true).expect("tight total");
    assert_eq!(tight.effective_total_length(), 1_000);
    let loose = ListItemSpecV2::new(common("l"), 0, 50, 500, 1_000_000, true).expect("loose total");
    assert_eq!(loose.effective_total_length(), 25_000);

    // Procedure v2 reuses the current first-version item contract taxonomy.
    assert_eq!(item("confirm").item_type(), ItemTypeV1::Confirm);
}

#[test]
fn v2grd003_conditions_are_bounded_typed_and_derived_from_one_snapshot() {
    let condition = ItemConditionV2::new(vec![ItemPredicateV2::new(
        ItemId::new("mode").unwrap(),
        false,
        ItemPredicateOperatorV2::Equals(PredicateScalarV2::text("strict").unwrap()),
    )])
    .unwrap();
    let mode = ItemSpecV2::choice(
        common("mode"),
        vec!["strict".to_owned(), "relaxed".to_owned()],
    )
    .unwrap();
    let notes = ItemSpecV2::text(
        ItemCommonV2::new(ItemId::new("notes").unwrap(), "Notes", None, false)
            .unwrap()
            .with_required_when(Some(condition))
            .unwrap(),
        1,
        100,
        true,
    )
    .unwrap();
    let items = vec![mode, notes];

    let strict = RecordedItemValueV2::choice("strict").unwrap();
    let strict_values = vec![Some(&strict), None];
    let status = items[1].condition_status(&items, &strict_values).unwrap();
    assert_eq!(status.state(), ConditionStateV2::Met);
    assert!(items[1].required_now(&items, &strict_values));

    let relaxed = RecordedItemValueV2::choice("relaxed").unwrap();
    let relaxed_values = vec![Some(&relaxed), None];
    assert_eq!(
        items[1]
            .condition_status(&items, &relaxed_values)
            .unwrap()
            .state(),
        ConditionStateV2::Unmet
    );
    assert!(!items[1].required_now(&items, &relaxed_values));

    let missing_values = vec![None, None];
    assert_eq!(
        items[1]
            .condition_status(&items, &missing_values)
            .unwrap()
            .state(),
        ConditionStateV2::Unevaluable
    );
    assert!(!items[1].required_now(&items, &missing_values));

    assert!(ItemConditionV2::new(Vec::new()).is_err());
    assert!(
        ItemConditionV2::new(
            (0..5)
                .map(|_| {
                    ItemPredicateV2::new(
                        ItemId::new("mode").unwrap(),
                        false,
                        ItemPredicateOperatorV2::Empty,
                    )
                })
                .collect(),
        )
        .is_err()
    );
    assert!(
        ItemCommonV2::new(ItemId::new("bad").unwrap(), "Bad", None, true)
            .unwrap()
            .with_required_when(Some(
                ItemConditionV2::new(vec![ItemPredicateV2::new(
                    ItemId::new("mode").unwrap(),
                    false,
                    ItemPredicateOperatorV2::Empty,
                )])
                .unwrap(),
            ))
            .is_err()
    );
}

#[test]
fn v2grd003_predicate_statuses_cover_counts_ranges_and_check_outcomes_without_content_echo() {
    let text = RecordedItemValueV2::text(" \u{2003} ").unwrap();
    let empty = ItemPredicateV2::new(
        ItemId::new("text").unwrap(),
        false,
        ItemPredicateOperatorV2::Empty,
    )
    .evaluate(Some(&text));
    assert_eq!(empty.state(), ConditionStateV2::Met);
    assert_eq!(empty.actual(), &PredicateActualV2::Count(0));

    let integer = RecordedItemValueV2::integer(i64::MIN);
    let boundary = ItemPredicateV2::new(
        ItemId::new("count").unwrap(),
        false,
        ItemPredicateOperatorV2::AtLeast(i64::MIN),
    )
    .evaluate(Some(&integer));
    assert_eq!(boundary.state(), ConditionStateV2::Met);
    assert_eq!(
        boundary.actual(),
        &PredicateActualV2::Scalar(PredicateScalarV2::Integer(i64::MIN))
    );

    let result = check_result("make-test", CheckResultOutcomeV2::Inconclusive);
    let outcome = ItemPredicateV2::new(
        ItemId::new("verification").unwrap(),
        true,
        ItemPredicateOperatorV2::NotEquals(PredicateScalarV2::text("fail").unwrap()),
    )
    .evaluate(Some(&result));
    assert_eq!(outcome.state(), ConditionStateV2::Met);
    assert_eq!(
        outcome.actual(),
        &PredicateActualV2::Scalar(PredicateScalarV2::Text("inconclusive".to_owned()))
    );
}

#[test]
fn integer_item_spec_enforces_range_order() {
    assert!(IntegerItemSpecV2::new(common("int"), None, None).is_ok());
    let ranged = IntegerItemSpecV2::new(common("int"), Some(-1), Some(1)).unwrap();
    assert_eq!(ranged.minimum(), Some(-1));
    assert_eq!(ranged.maximum(), Some(1));
    // equal bounds are permitted; reversed bounds are rejected.
    assert!(IntegerItemSpecV2::new(common("int"), Some(5), Some(5)).is_ok());
    assert_eq!(
        IntegerItemSpecV2::new(common("int"), Some(2), Some(1)).unwrap_err(),
        invalid("integer minimum must not exceed maximum")
    );
}

#[test]
fn artifact_item_spec_enforces_count_format_and_uniqueness() {
    let artifact = ArtifactItemSpecV2::new(common("art"), vec!["text/plain".to_owned()]).unwrap();
    assert_eq!(artifact.allowed_media_types(), &["text/plain".to_owned()]);

    let too_many_media: Vec<String> = (0..65).map(|i| format!("t{i}/plain")).collect();
    assert_eq!(
        ArtifactItemSpecV2::new(common("art"), too_many_media).unwrap_err(),
        invalid("too many allowed media types")
    );
    assert_eq!(
        ArtifactItemSpecV2::new(
            common("art"),
            vec!["text/plain".to_owned(), "text/plain".to_owned()],
        )
        .unwrap_err(),
        invalid("allowed media types must be unique")
    );
    // uppercase kind is rejected by the shared media-type format check.
    assert_eq!(
        ArtifactItemSpecV2::new(common("art"), vec!["Not/Lower".to_owned()]).unwrap_err(),
        invalid("media type must be lowercase ASCII without parameters")
    );
}

#[test]
fn check_result_declarations_are_closed_bounded_and_unique() {
    let specification = CheckResultItemSpecV2::new(
        common("verification"),
        OperationId::new("make-test").unwrap(),
        digest('a'),
        vec![
            CheckResultOutcomeV2::Pass,
            CheckResultOutcomeV2::Inconclusive,
        ],
    )
    .unwrap();
    assert_eq!(specification.operation_id().as_str(), "make-test");
    assert_eq!(specification.operation_digest(), &digest('a'));
    assert_eq!(
        specification.accepted_outcomes(),
        &[
            CheckResultOutcomeV2::Pass,
            CheckResultOutcomeV2::Inconclusive
        ]
    );
    assert_eq!(
        CheckResultItemSpecV2::new(
            common("verification"),
            OperationId::new("make-test").unwrap(),
            digest('a'),
            Vec::new(),
        )
        .unwrap_err(),
        invalid("check result accepted outcomes must contain between one and three values")
    );
    assert_eq!(
        CheckResultItemSpecV2::new(
            common("verification"),
            OperationId::new("make-test").unwrap(),
            digest('a'),
            vec![CheckResultOutcomeV2::Pass, CheckResultOutcomeV2::Pass],
        )
        .unwrap_err(),
        invalid("check result accepted outcomes must be unique")
    );
}

#[test]
fn check_result_values_enforce_every_scalar_bound() {
    assert!(CheckResultInputBasisV2::new("x".repeat(512), digest('b')).is_ok());
    assert!(CheckResultInputBasisV2::new("x".repeat(513), digest('b')).is_err());
    assert!(CheckResultExecutorV2::new("x".repeat(128), "v".repeat(64)).is_ok());
    assert!(CheckResultExecutorV2::new("x".repeat(129), "v").is_err());
    assert!(CheckResultExecutorV2::new("runner", "v".repeat(65)).is_err());

    let value = |summary: String| {
        CheckResultValueV2::new(
            OperationId::new("make-test").unwrap(),
            digest('a'),
            CheckResultInputBasisV2::new("basis", digest('b')).unwrap(),
            CheckResultExecutorV2::new("gaori", "unknown").unwrap(),
            CheckResultOutcomeV2::Fail,
            summary,
            digest('c'),
        )
    };
    assert!(value("x".repeat(2_000)).is_ok());
    assert!(value("x".repeat(2_001)).is_err());
    assert!(CheckResultExecutorV2::new("   ", "unknown").is_err());
}

#[test]
fn item_specs_admit_only_recorded_values_that_satisfy_the_declaration() {
    let text = ItemSpecV2::text(common("text"), 2, 4, true).unwrap();
    assert!(text.admits_recorded_value(&RecordedItemValueV2::text("okay").unwrap()));
    assert!(text.admits_recorded_value(&RecordedItemValueV2::text(" okay ").unwrap()));
    assert!(!text.admits_recorded_value(&RecordedItemValueV2::text("x").unwrap()));
    assert!(!text.admits_recorded_value(&RecordedItemValueV2::text(" x ").unwrap()));
    assert!(!text.admits_recorded_value(&RecordedItemValueV2::text(" \u{2003} ").unwrap()));

    let choice = ItemSpecV2::choice(common("choice"), vec!["green".to_owned()]).unwrap();
    assert!(choice.admits_recorded_value(&RecordedItemValueV2::choice("green").unwrap()));
    assert!(!choice.admits_recorded_value(&RecordedItemValueV2::choice("blue").unwrap()));

    let integer = ItemSpecV2::integer(common("integer"), Some(2), Some(4)).unwrap();
    assert!(integer.admits_recorded_value(&RecordedItemValueV2::integer(3)));
    assert!(!integer.admits_recorded_value(&RecordedItemValueV2::integer(5)));

    let list = ItemSpecV2::list(common("list"), 1, 2, 3, 1_000_000, true).unwrap();
    assert!(list.admits_recorded_value(
        &RecordedItemValueV2::list(vec!["one".to_owned(), "two".to_owned()]).unwrap()
    ));
    assert!(!list.admits_recorded_value(
        &RecordedItemValueV2::list(vec!["one".to_owned(), "one".to_owned()]).unwrap()
    ));

    // A recorded list must satisfy the total-content ceiling as well as the per-entry ceiling.
    let bounded_total = ItemSpecV2::list(common("list"), 1, 4, 8, 5, true).unwrap();
    assert!(bounded_total.admits_recorded_value(
        &RecordedItemValueV2::list(vec!["ab".to_owned(), "cde".to_owned()]).unwrap()
    ));
    assert!(!bounded_total.admits_recorded_value(
        &RecordedItemValueV2::list(vec!["abc".to_owned(), "defg".to_owned()]).unwrap()
    ));
    assert_eq!(
        bounded_total.recorded_value_bound_error(
            &RecordedItemValueV2::list(vec!["abc".to_owned(), "defg".to_owned()]).unwrap()
        ),
        Some(DomainError::BoundExceeded {
            field: "item list content",
            actual: 7,
            maximum: 5,
            unit: BoundUnitV2::Scalars,
        })
    );

    let artifact = ItemSpecV2::artifact(common("artifact"), vec!["text/plain".to_owned()]).unwrap();
    let value = RecordedItemValueV2::artifact(
        ArtifactValueV1::external_reference(
            "urn:example:result",
            Sha256Digest::new(format!("sha256:{}", "a".repeat(64))).unwrap(),
            1,
            "application/json",
        )
        .unwrap(),
    );
    assert!(!artifact.admits_recorded_value(&value));

    let check = ItemSpecV2::check_result(
        common("verification"),
        OperationId::new("make-test").unwrap(),
        digest('a'),
        vec![
            CheckResultOutcomeV2::Pass,
            CheckResultOutcomeV2::Inconclusive,
        ],
    )
    .unwrap();
    assert!(check.is_satisfied_by(&check_result("make-test", CheckResultOutcomeV2::Pass)));
    assert!(check.is_satisfied_by(&check_result(
        "make-test",
        CheckResultOutcomeV2::Inconclusive
    )));
    assert!(!check.is_satisfied_by(&check_result("make-test", CheckResultOutcomeV2::Fail)));
    assert!(!check.is_satisfied_by(&check_result("other-test", CheckResultOutcomeV2::Pass)));
    let wrong_digest = RecordedItemValueV2::check_result(
        CheckResultValueV2::new(
            OperationId::new("make-test").unwrap(),
            digest('d'),
            CheckResultInputBasisV2::new("basis", digest('b')).unwrap(),
            CheckResultExecutorV2::new("gaori", "1.0.0").unwrap(),
            CheckResultOutcomeV2::Pass,
            "Passed.",
            digest('c'),
        )
        .unwrap(),
    );
    assert!(!check.is_satisfied_by(&wrong_digest));
    assert!(check.admits_recorded_value(&wrong_digest));
}
