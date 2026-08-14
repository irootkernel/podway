use crate::{CriterionId, DomainError, GraphNodeId, NodeDefinitionId, OptionId};

mod definitions;
mod goal;
mod graph;
mod helpers;
mod items;

#[test]
fn identifiers_enforce_shared_kebab_bounds() {
    assert!(NodeDefinitionId::new("a").is_ok());
    assert!(GraphNodeId::new("implement-change-2").is_ok());
    assert_eq!(
        GraphNodeId::new(""),
        Err(DomainError::EmptyValue {
            field: "GraphNodeId"
        })
    );
    let at_limit = "a".repeat(64);
    assert!(OptionId::new(at_limit).is_ok());
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
