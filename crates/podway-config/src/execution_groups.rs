//! Internal successor authoring contracts. These APIs do not change public v2 dispatch.
use crate::procedure_v2_document::AuthoringValue;
use crate::{CanonicalJsonV1, ConfigError};
use podway_core::execution_groups::*;
use podway_core::{ItemSpecV2, Sha256Digest};

mod parse;
mod projection;
mod validate;
pub use parse::parse_successor_document;
pub use projection::{
    FormattedSuccessorDefinition, SuccessorGraphProjection, format_successor_definition,
    project_retained_successor_graph, project_successor_graph, project_successor_source,
};
pub use validate::{validate_group_interface, validate_successor_definition};

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SuccessorDefinition {
    Base(ProcedureDefinitionV3),
    Group(ExecutionGroupDefinitionV1),
}
impl SuccessorDefinition {
    pub fn header(&self) -> &DefinitionHeaderV3 {
        match self {
            Self::Base(v) => v.header(),
            Self::Group(v) => v.header(),
        }
    }
    pub fn definitions(&self) -> &[NodeDefinitionV3] {
        match self {
            Self::Base(v) => v.definitions(),
            Self::Group(v) => v.definitions(),
        }
    }
    pub fn graph(&self) -> &TopologyV3 {
        match self {
            Self::Base(v) => v.graph(),
            Self::Group(v) => v.graph(),
        }
    }
    pub const fn schema(&self) -> &'static str {
        match self {
            Self::Base(_) => PROCEDURE_SCHEMA_V3,
            Self::Group(_) => EXECUTION_GROUP_SCHEMA_V1,
        }
    }
}

/// Constructible only after complete closed-reference, graph and boundary admission.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedSuccessorDefinition {
    definition: SuccessorDefinition,
    authoring: AuthoringValue,
    canonical_json: CanonicalJsonV1,
    digest: Sha256Digest,
}
impl ValidatedSuccessorDefinition {
    pub fn definition(&self) -> &SuccessorDefinition {
        &self.definition
    }
    pub fn canonical_json(&self) -> &CanonicalJsonV1 {
        &self.canonical_json
    }
    pub fn digest(&self) -> &Sha256Digest {
        &self.digest
    }
}

fn invalid(field: &'static str, reason: &'static str) -> ConfigError {
    ConfigError::InvalidValue { field, reason }
}
fn core(error: podway_core::DomainError) -> ConfigError {
    crate::procedure_v2_parse::map_domain_error(error)
}
fn items(definition: &NodeDefinitionV3) -> &[ItemSpecV2] {
    match definition {
        NodeDefinitionV3::Action(v) => v.items(),
        NodeDefinitionV3::Decision(v) => v.metadata().items(),
    }
}
