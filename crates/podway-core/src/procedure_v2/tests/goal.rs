use crate::procedure_v2::invalid;
use crate::{
    CriterionAssessmentReasonV2, CriterionId, GoalCriterionV2, GoalDefinitionV2,
    GoalRevisionReasonV2, GoalStatementV2, GoalTrackingOptIn,
};

fn criterion_id(value: &str) -> CriterionId {
    CriterionId::new(value).unwrap()
}

#[test]
fn goal_tracking_opt_in_accepts_only_true() {
    assert!(GoalTrackingOptIn::from_bool(true).unwrap().is_enabled());
    assert_eq!(
        GoalTrackingOptIn::from_bool(false),
        Err(invalid(
            "goal_tracking accepts only true; omit the key to disable"
        ))
    );
    assert!(GoalTrackingOptIn::enabled().is_enabled());
}

#[test]
fn goal_statement_enforces_non_empty_unicode_char_bound() {
    // Each '가' is one Unicode scalar value but three UTF-8 bytes; the bound counts characters.
    let at_limit = "가".repeat(1_000);
    let statement = GoalStatementV2::new(at_limit.clone()).unwrap();
    assert_eq!(statement.as_str(), at_limit.as_str());
    assert_eq!(
        GoalStatementV2::new("가".repeat(1_001)),
        Err(invalid("goal statement"))
    );
    assert_eq!(GoalStatementV2::new(""), Err(invalid("goal statement")));
    assert_eq!(GoalStatementV2::new("   "), Err(invalid("goal statement")));
}

#[test]
fn goal_criterion_enforces_unicode_statement_bound() {
    let criterion = GoalCriterionV2::new(criterion_id("deterministic"), "가".repeat(300)).unwrap();
    assert_eq!(criterion.id().as_str(), "deterministic");
    assert_eq!(criterion.statement(), "가".repeat(300).as_str());
    assert_eq!(
        GoalCriterionV2::new(criterion_id("deterministic"), "가".repeat(301)).unwrap_err(),
        invalid("criterion statement")
    );
    assert_eq!(
        GoalCriterionV2::new(criterion_id("deterministic"), "").unwrap_err(),
        invalid("criterion statement")
    );
}

#[test]
fn goal_definition_enforces_criteria_bounds_unique_ids_and_order() {
    fn criterion(id: &str) -> GoalCriterionV2 {
        GoalCriterionV2::new(criterion_id(id), format!("Statement for {id}")).unwrap()
    }

    let one = GoalDefinitionV2::new(vec![criterion("c1")]).unwrap();
    assert_eq!(one.criteria().len(), 1);

    let sixteen: Vec<GoalCriterionV2> = (0..16).map(|i| criterion(&format!("c{i}"))).collect();
    assert!(GoalDefinitionV2::new(sixteen).is_ok());

    assert_eq!(
        GoalDefinitionV2::new(Vec::new()).unwrap_err(),
        invalid("goal criterion count must be between one and 16")
    );
    let seventeen: Vec<GoalCriterionV2> = (0..17).map(|i| criterion(&format!("c{i}"))).collect();
    assert_eq!(
        GoalDefinitionV2::new(seventeen).unwrap_err(),
        invalid("goal criterion count must be between one and 16")
    );

    let duplicate = vec![criterion("dup"), criterion("dup")];
    assert_eq!(
        GoalDefinitionV2::new(duplicate).unwrap_err(),
        invalid("goal criterion identifiers must be unique")
    );

    // author order is preserved, independent of identifier lexicographic order.
    let ordered = GoalDefinitionV2::new(vec![
        criterion("gamma"),
        criterion("alpha"),
        criterion("beta"),
    ])
    .unwrap();
    let ids: Vec<&str> = ordered.criteria().iter().map(|c| c.id().as_str()).collect();
    assert_eq!(ids, vec!["gamma", "alpha", "beta"]);
}

#[test]
fn goal_revision_reason_enforces_non_empty_unicode_bound() {
    assert!(GoalRevisionReasonV2::new("가".repeat(1_000)).is_ok());
    assert_eq!(
        GoalRevisionReasonV2::new("가".repeat(1_001)).unwrap_err(),
        invalid("goal revision reason")
    );
    assert_eq!(
        GoalRevisionReasonV2::new("").unwrap_err(),
        invalid("goal revision reason")
    );
    assert_eq!(
        GoalRevisionReasonV2::new("  ").unwrap_err(),
        invalid("goal revision reason")
    );
}

#[test]
fn criterion_assessment_reason_enforces_non_empty_unicode_bound() {
    assert!(CriterionAssessmentReasonV2::new("가".repeat(2_000)).is_ok());
    assert_eq!(
        CriterionAssessmentReasonV2::new("가".repeat(2_001)).unwrap_err(),
        invalid("criterion assessment reason")
    );
    assert_eq!(
        CriterionAssessmentReasonV2::new("").unwrap_err(),
        invalid("criterion assessment reason")
    );
}
