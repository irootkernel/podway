use crate::procedure_v2::invalid;
use crate::{
    ArtifactItemSpecV2, ChoiceItemSpecV2, IntegerItemSpecV2, ItemCommonV2, ItemId, ItemTypeV1,
    ListItemSpecV2, TextItemSpecV2,
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

    // list bounds cap at 100 entries of 1_000 characters.
    assert!(ListItemSpecV2::new(common("l"), 0, 100, 1_000, true).is_ok());
    assert_eq!(
        ListItemSpecV2::new(common("l"), 0, 101, 500, true).unwrap_err(),
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
