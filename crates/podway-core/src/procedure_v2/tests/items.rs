use crate::procedure_v2::invalid;
use crate::{
    ArtifactItemSpecV2, ArtifactValueV1, ChoiceItemSpecV2, IntegerItemSpecV2, ItemCommonV2, ItemId,
    ItemSpecV2, ItemTypeV1, ListItemSpecV2, RecordedItemValueV2, Sha256Digest, TextItemSpecV2,
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
}
