use crate::{CriterionId, DomainError, GraphNodeId, NodeDefinitionId, OptionId};

mod definitions;
mod goal;
mod graph;
mod helpers;
mod items;

#[test]
fn identifiers_enforce_kebab_bounds_and_reuse_v1_rule() {
    assert!(NodeDefinitionId::new("a").is_ok());
    assert!(GraphNodeId::new("implement-change-2").is_ok());
    assert_eq!(
        GraphNodeId::new(""),
        Err(DomainError::EmptyValue {
            field: "GraphNodeId"
        })
    );
    let overlong = "a".repeat(65);
    assert_eq!(
        OptionId::new(overlong.clone()),
        Err(DomainError::ValueTooLong {
            field: "OptionId",
            maximum: 64,
            actual: 65,
        })
    );
    assert_eq!(
        CriterionId::new("Bad-Case"),
        Err(DomainError::InvalidIdentifier {
            field: "CriterionId"
        })
    );
    assert_eq!(
        CriterionId::new("trailing-"),
        Err(DomainError::InvalidIdentifier {
            field: "CriterionId"
        })
    );
}
