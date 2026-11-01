use serde::{Deserialize, Serialize};

use crate::{
    CanonicalJson, CanonicalJsonV1, ConfigError, ProcedureDocumentFormat, ProcedureDocumentLimits,
    WorkspaceConfigParseLimitsV1, canonical_json_from_serializable,
    decode_procedure_document_with_limits,
};

pub const USER_CONFIG_SCHEMA_V1: &str = "podway.user-config/v1";

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DolgoraeIntegrationV1 {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IntegrationSettingsV1 {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dolgorae: Option<DolgoraeIntegrationV1>,
}

impl IntegrationSettingsV1 {
    pub fn dolgorae_enabled(&self) -> Option<bool> {
        self.dolgorae.as_ref().and_then(|value| value.enabled)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserConfigV1 {
    pub schema: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub integrations: Option<IntegrationSettingsV1>,
}

impl CanonicalJson for UserConfigV1 {
    fn canonical_json_v1(&self) -> Result<CanonicalJsonV1, ConfigError> {
        self.validate()?;
        canonical_json_from_serializable(self)
    }
}

impl UserConfigV1 {
    pub fn validate(&self) -> Result<(), ConfigError> {
        if self.schema != USER_CONFIG_SCHEMA_V1 {
            return Err(ConfigError::InvalidSchema {
                expected: USER_CONFIG_SCHEMA_V1,
                actual: self.schema.clone(),
            });
        }
        Ok(())
    }
}

pub fn parse_user_config_v1(input: impl AsRef<[u8]>) -> Result<UserConfigV1, ConfigError> {
    parse_user_config_v1_with_limits(input, WorkspaceConfigParseLimitsV1::default())
}

pub fn parse_user_config_v1_with_limits(
    input: impl AsRef<[u8]>,
    limits: WorkspaceConfigParseLimitsV1,
) -> Result<UserConfigV1, ConfigError> {
    let input = input.as_ref();
    decode_procedure_document_with_limits(
        input,
        ProcedureDocumentFormat::Yaml,
        ProcedureDocumentLimits {
            max_bytes: limits.max_bytes,
            max_depth: limits.max_depth,
            max_nodes: limits.max_nodes,
        },
    )?;
    let config: UserConfigV1 =
        serde_norway::from_str(std::str::from_utf8(input).map_err(|_| {
            ConfigError::InvalidDocument {
                reason: "input must be valid UTF-8".to_owned(),
            }
        })?)
        .map_err(|error| ConfigError::InvalidDocument {
            reason: error.to_string(),
        })?;
    config.validate()?;
    Ok(config)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IntegrationSettingsSourceV1 {
    Project,
    User,
    Default,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResolvedIntegrationSettingsV1 {
    pub enabled: bool,
    pub source: IntegrationSettingsSourceV1,
}

pub fn resolve_integration_settings_v1(
    project: Option<bool>,
    user: Option<bool>,
) -> ResolvedIntegrationSettingsV1 {
    let (enabled, source) = if let Some(value) = project {
        (value, IntegrationSettingsSourceV1::Project)
    } else if let Some(value) = user {
        (value, IntegrationSettingsSourceV1::User)
    } else {
        (false, IntegrationSettingsSourceV1::Default)
    };
    ResolvedIntegrationSettingsV1 { enabled, source }
}
