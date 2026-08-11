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
fn item_specs_enforce_v2_bounds_while_keeping_v1_unchanged() {
    // v2 item prompt is capped at 300 characters (v1 keeps 500).
    assert!(ItemCommonV2::new(ItemId::new("i").unwrap(), "p".repeat(300), None, true).is_ok());
    assert_eq!(
        ItemCommonV2::new(ItemId::new("i").unwrap(), "p".repeat(301), None, true).unwrap_err(),
        invalid("item prompt")
    );
    // v1 item prompt bound stays at 500 (no drift).
    assert!(
        crate::ItemCommonV1::new(ItemId::new("i").unwrap(), "p".repeat(500), None, true).is_ok()
    );
    assert_eq!(
        crate::ItemCommonV1::new(ItemId::new("i").unwrap(), "p".repeat(501), None, true)
            .unwrap_err(),
        crate::DomainError::InvalidState {
            reason: "item prompt"
        }
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

    // text max length hard cap is 16_384; one over fails.
    assert!(TextItemSpecV2::new(common("t"), 0, 16_384, true).is_ok());
    assert_eq!(
        TextItemSpecV2::new(common("t"), 0, 16_385, true).unwrap_err(),
        invalid("invalid text length constraints")
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

    // list bounds cap at 200 entries of 1_000 characters.
    assert!(ListItemSpecV2::new(common("l"), 0, 200, 1_000, true).is_ok());
    assert_eq!(
        ListItemSpecV2::new(common("l"), 0, 201, 500, true).unwrap_err(),
        invalid("invalid list item count constraints")
    );
    assert_eq!(
        ListItemSpecV2::new(common("l"), 0, 50, 1_001, true).unwrap_err(),
        invalid("invalid list entry length constraint")
    );

    // item kind taxonomy is reused from the v1 item contracts.
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

    let list = ItemSpecV2::list(common("list"), 1, 2, 3, true).unwrap();
    assert!(list.admits_recorded_value(
        &RecordedItemValueV2::list(vec!["one".to_owned(), "two".to_owned()]).unwrap()
    ));
    assert!(!list.admits_recorded_value(
        &RecordedItemValueV2::list(vec!["one".to_owned(), "one".to_owned()]).unwrap()
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
