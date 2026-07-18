use std::{
    cell::{Cell, RefCell},
    collections::BTreeSet,
    fmt,
};

use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
use serde_json::{Map, Number, Value};
use yaml_rust2::{
    Yaml,
    scanner::{Scanner, TScalarStyle, TokenType},
};

use crate::{ConfigError, ProcedureDefinitionV1, ValidatedProcedureV1, WorkspaceConfigV1};

pub const MAX_PROCEDURE_DOCUMENT_BYTES_V1: usize = podway_core::MAX_PROCEDURE_DOCUMENT_BYTES_V1;
pub const MAX_PROCEDURE_DOCUMENT_DEPTH_V1: usize = 64;
pub const MAX_PROCEDURE_DOCUMENT_NODES_V1: usize = 100_000;
pub const MAX_WORKSPACE_CONFIG_BYTES_V1: usize = 64 * 1024;
pub const MAX_WORKSPACE_CONFIG_DEPTH_V1: usize = 16;
pub const MAX_WORKSPACE_CONFIG_NODES_V1: usize = 1_024;

/// The source encoding accepted for a procedure v1 document.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProcedureFormatV1 {
    Json,
    Yaml,
}

/// Resource limits applied before a procedure is admitted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProcedureParseLimitsV1 {
    pub max_bytes: usize,
    pub max_depth: usize,
    pub max_nodes: usize,
}

impl Default for ProcedureParseLimitsV1 {
    fn default() -> Self {
        Self {
            max_bytes: MAX_PROCEDURE_DOCUMENT_BYTES_V1,
            max_depth: MAX_PROCEDURE_DOCUMENT_DEPTH_V1,
            max_nodes: MAX_PROCEDURE_DOCUMENT_NODES_V1,
        }
    }
}
/// Resource limits applied before a workspace config is admitted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorkspaceConfigParseLimitsV1 {
    pub max_bytes: usize,
    pub max_depth: usize,
    pub max_nodes: usize,
}

impl Default for WorkspaceConfigParseLimitsV1 {
    fn default() -> Self {
        Self {
            max_bytes: MAX_WORKSPACE_CONFIG_BYTES_V1,
            max_depth: MAX_WORKSPACE_CONFIG_DEPTH_V1,
            max_nodes: MAX_WORKSPACE_CONFIG_NODES_V1,
        }
    }
}

#[derive(Clone, Copy)]
struct BoundedYamlLimitsV1 {
    max_depth: usize,
    max_nodes: usize,
}

impl From<ProcedureParseLimitsV1> for BoundedYamlLimitsV1 {
    fn from(limits: ProcedureParseLimitsV1) -> Self {
        Self {
            max_depth: limits.max_depth,
            max_nodes: limits.max_nodes,
        }
    }
}

impl From<WorkspaceConfigParseLimitsV1> for BoundedYamlLimitsV1 {
    fn from(limits: WorkspaceConfigParseLimitsV1) -> Self {
        Self {
            max_depth: limits.max_depth,
            max_nodes: limits.max_nodes,
        }
    }
}

/// Parses, default-expands, and semantically validates a workspace config v1 document.
pub fn parse_workspace_config_v1(
    input: impl AsRef<[u8]>,
) -> Result<WorkspaceConfigV1, ConfigError> {
    parse_workspace_config_v1_with_limits(input, WorkspaceConfigParseLimitsV1::default())
}

/// As [`parse_workspace_config_v1`], with explicit resource limits for a trusted caller's
/// boundary.
pub fn parse_workspace_config_v1_with_limits(
    input: impl AsRef<[u8]>,
    limits: WorkspaceConfigParseLimitsV1,
) -> Result<WorkspaceConfigV1, ConfigError> {
    let input = input.as_ref();
    if input.len() > limits.max_bytes {
        return Err(ConfigError::InputTooLarge {
            maximum: limits.max_bytes,
            actual: input.len(),
        });
    }
    let text = std::str::from_utf8(input).map_err(|_| ConfigError::InvalidDocument {
        reason: "input must be valid UTF-8".to_owned(),
    })?;

    let config = parse_yaml_workspace_config(text, limits)?;
    config.validate()?;
    Ok(config)
}

/// Parses, default-expands, canonicalizes, and semantically validates a procedure v1 document.
pub fn parse_procedure_v1(
    input: impl AsRef<[u8]>,
    format: ProcedureFormatV1,
) -> Result<ValidatedProcedureV1, ConfigError> {
    parse_procedure_v1_with_limits(input, format, ProcedureParseLimitsV1::default())
}

/// As [`parse_procedure_v1`], with explicit resource limits for a trusted caller's boundary.
pub fn parse_procedure_v1_with_limits(
    input: impl AsRef<[u8]>,
    format: ProcedureFormatV1,
    limits: ProcedureParseLimitsV1,
) -> Result<ValidatedProcedureV1, ConfigError> {
    let input = input.as_ref();
    if input.len() > limits.max_bytes {
        return Err(ConfigError::InputTooLarge {
            maximum: limits.max_bytes,
            actual: input.len(),
        });
    }
    let text = std::str::from_utf8(input).map_err(|_| ConfigError::InvalidDocument {
        reason: "input must be valid UTF-8".to_owned(),
    })?;

    let mut definition = match format {
        ProcedureFormatV1::Json => {
            let value = JsonParser::new(text, limits).parse()?;
            reject_nulls(&value)?;
            serde_json::from_value(value).map_err(|error| ConfigError::InvalidDocument {
                reason: error.to_string(),
            })?
        }
        ProcedureFormatV1::Yaml => parse_yaml_definition(text, limits)?,
    };

    definition.validate()?;
    definition.apply_documented_defaults();
    ValidatedProcedureV1::new(definition)
}

fn parse_yaml_definition(
    input: &str,
    limits: ProcedureParseLimitsV1,
) -> Result<ProcedureDefinitionV1, ConfigError> {
    if input.trim().is_empty() {
        return Err(ConfigError::InvalidDocument {
            reason: "procedure document must not be empty".to_owned(),
        });
    }
    preflight_yaml(input)?;

    let nodes = Cell::new(0usize);
    let failure = RefCell::new(None);
    let mut documents = serde_yaml::Deserializer::from_str(input);
    let Some(document) = documents.next() else {
        return Err(ConfigError::InvalidDocument {
            reason: "procedure document must not be empty".to_owned(),
        });
    };
    if let Err(error) = (BoundedYamlSeed {
        depth: 1,
        nodes: &nodes,
        limits: limits.into(),
        document_name: "procedure v1",
        failure: &failure,
    })
    .deserialize(document)
    {
        return Err(failure
            .into_inner()
            .unwrap_or_else(|| ConfigError::InvalidDocument {
                reason: error.to_string(),
            }));
    }
    if documents.next().is_some() {
        return Err(ConfigError::InvalidDocument {
            reason: "procedure document must contain exactly one YAML document".to_owned(),
        });
    }

    // Deserialize from the original event stream so serde's derived struct deserializers reject
    // duplicate fields rather than allowing an intermediate map to overwrite one.
    serde_yaml::from_str(input).map_err(|error| ConfigError::InvalidDocument {
        reason: error.to_string(),
    })
}
fn parse_yaml_workspace_config(
    input: &str,
    limits: WorkspaceConfigParseLimitsV1,
) -> Result<WorkspaceConfigV1, ConfigError> {
    if input.trim().is_empty() {
        return Err(ConfigError::InvalidDocument {
            reason: "workspace config document must not be empty".to_owned(),
        });
    }
    preflight_yaml(input)?;

    let nodes = Cell::new(0usize);
    let failure = RefCell::new(None);
    let mut documents = serde_yaml::Deserializer::from_str(input);
    let Some(document) = documents.next() else {
        return Err(ConfigError::InvalidDocument {
            reason: "workspace config document must not be empty".to_owned(),
        });
    };
    if let Err(error) = (BoundedYamlSeed {
        depth: 1,
        nodes: &nodes,
        limits: limits.into(),
        document_name: "workspace config v1",
        failure: &failure,
    })
    .deserialize(document)
    {
        return Err(failure
            .into_inner()
            .unwrap_or_else(|| ConfigError::InvalidDocument {
                reason: error.to_string(),
            }));
    }
    if documents.next().is_some() {
        return Err(ConfigError::InvalidDocument {
            reason: "workspace config document must contain exactly one YAML document".to_owned(),
        });
    }

    serde_yaml::from_str(input).map_err(|error| ConfigError::InvalidDocument {
        reason: error.to_string(),
    })
}
fn reject_nulls(value: &Value) -> Result<(), ConfigError> {
    match value {
        Value::Null => Err(ConfigError::InvalidDocument {
            reason: "explicit null is not allowed by procedure v1".to_owned(),
        }),
        Value::Array(values) => {
            for value in values {
                reject_nulls(value)?;
            }
            Ok(())
        }
        Value::Object(values) => {
            for value in values.values() {
                reject_nulls(value)?;
            }
            Ok(())
        }
        Value::Bool(_) | Value::Number(_) | Value::String(_) => Ok(()),
    }
}

#[derive(Clone, Copy)]
struct BoundedYamlSeed<'a> {
    depth: usize,
    nodes: &'a Cell<usize>,
    limits: BoundedYamlLimitsV1,
    document_name: &'static str,
    failure: &'a RefCell<Option<ConfigError>>,
}

impl BoundedYamlSeed<'_> {
    fn child(self) -> Self {
        Self {
            depth: self.depth.saturating_add(1),
            ..self
        }
    }

    fn count_node<E: de::Error>(self) -> Result<(), E> {
        if self.depth > self.limits.max_depth {
            return Err(self.reject(ConfigError::InputTooDeep {
                maximum: self.limits.max_depth,
                actual: self.depth,
            }));
        }

        let actual = self.nodes.get().saturating_add(1);
        if actual > self.limits.max_nodes {
            return Err(self.reject(ConfigError::InputTooComplex {
                maximum: self.limits.max_nodes,
                actual,
            }));
        }
        self.nodes.set(actual);
        Ok(())
    }

    fn reject<E: de::Error>(self, error: ConfigError) -> E {
        let reason = error.to_string();
        *self.failure.borrow_mut() = Some(error);
        E::custom(reason)
    }
}

impl<'de> DeserializeSeed<'de> for BoundedYamlSeed<'_> {
    type Value = ();

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for BoundedYamlSeed<'_> {
    type Value = ();

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a procedure v1 YAML value")
    }

    fn visit_bool<E>(self, _: bool) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.count_node()
    }

    fn visit_i64<E>(self, _: i64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.count_node()
    }

    fn visit_i128<E>(self, value: i128) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.count_node()?;
        if i64::try_from(value).is_ok() {
            Ok(())
        } else {
            Err(self.reject(ConfigError::NonCanonicalNumber))
        }
    }

    fn visit_u64<E>(self, _: u64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.count_node()
    }

    fn visit_u128<E>(self, value: u128) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.count_node()?;
        if u64::try_from(value).is_ok() {
            Ok(())
        } else {
            Err(self.reject(ConfigError::NonCanonicalNumber))
        }
    }

    fn visit_f32<E>(self, _: f32) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.count_node()?;
        Err(self.reject(ConfigError::NonCanonicalNumber))
    }

    fn visit_f64<E>(self, _: f64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.count_node()?;
        Err(self.reject(ConfigError::NonCanonicalNumber))
    }

    fn visit_char<E>(self, _: char) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.count_node()
    }

    fn visit_str<E>(self, _: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.count_node()
    }

    fn visit_borrowed_str<E>(self, _: &'de str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.count_node()
    }

    fn visit_string<E>(self, _: String) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.count_node()
    }

    fn visit_bytes<E>(self, _: &[u8]) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.count_node()?;
        Err(self.reject(ConfigError::InvalidDocument {
            reason: format!(
                "YAML binary values are not allowed by {}",
                self.document_name
            ),
        }))
    }

    fn visit_byte_buf<E>(self, _: Vec<u8>) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.count_node()?;
        Err(self.reject(ConfigError::InvalidDocument {
            reason: format!(
                "YAML binary values are not allowed by {}",
                self.document_name
            ),
        }))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.visit_unit()
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        self.count_node()?;
        Err(self.reject(ConfigError::InvalidDocument {
            reason: format!("explicit null is not allowed by {}", self.document_name),
        }))
    }

    fn visit_seq<A>(self, mut sequence: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        self.count_node()?;
        while let Some(()) = sequence.next_element_seed(self.child())? {}
        Ok(())
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        self.count_node()?;
        let mut keys = BTreeSet::new();
        while let Some(key) = map.next_key_seed(YamlMappingKeySeed {
            failure: self.failure,
        })? {
            if !keys.insert(key.clone()) {
                return Err(self.reject(ConfigError::DuplicateKey { key }));
            }
            map.next_value_seed(self.child())?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct YamlMappingKeySeed<'a> {
    failure: &'a RefCell<Option<ConfigError>>,
}

impl YamlMappingKeySeed<'_> {
    fn reject<E: de::Error>(self) -> E {
        let error = ConfigError::InvalidDocument {
            reason: "mapping keys must be strings".to_owned(),
        };
        let reason = error.to_string();
        *self.failure.borrow_mut() = Some(error);
        E::custom(reason)
    }
}

impl<'de> DeserializeSeed<'de> for YamlMappingKeySeed<'_> {
    type Value = String;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for YamlMappingKeySeed<'_> {
    type Value = String;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a string mapping key")
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(value.to_owned())
    }

    fn visit_borrowed_str<E>(self, value: &'de str) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(value.to_owned())
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(value)
    }

    fn visit_bool<E>(self, _: bool) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Err(self.reject())
    }

    fn visit_i64<E>(self, _: i64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Err(self.reject())
    }

    fn visit_i128<E>(self, _: i128) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Err(self.reject())
    }

    fn visit_u64<E>(self, _: u64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Err(self.reject())
    }

    fn visit_u128<E>(self, _: u128) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Err(self.reject())
    }

    fn visit_f32<E>(self, _: f32) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Err(self.reject())
    }

    fn visit_f64<E>(self, _: f64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Err(self.reject())
    }

    fn visit_char<E>(self, _: char) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Err(self.reject())
    }

    fn visit_bytes<E>(self, _: &[u8]) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Err(self.reject())
    }

    fn visit_byte_buf<E>(self, _: Vec<u8>) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Err(self.reject())
    }

    fn visit_none<E>(self) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Err(self.reject())
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Err(self.reject())
    }

    fn visit_seq<A>(self, _: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        Err(self.reject())
    }

    fn visit_map<A>(self, _: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        Err(self.reject())
    }
}

fn preflight_yaml(input: &str) -> Result<(), ConfigError> {
    let source_characters = input.chars().collect::<Vec<_>>();
    let mut scanner = Scanner::new(input.chars());
    let mut mapping_key_scalar = false;

    for token in scanner.by_ref() {
        match token.1 {
            TokenType::Alias(_) => {
                return Err(ConfigError::UnsupportedYamlFeature { feature: "alias" });
            }
            TokenType::Anchor(_) => {
                return Err(ConfigError::UnsupportedYamlFeature { feature: "anchor" });
            }
            TokenType::Tag(_, _)
            | TokenType::TagDirective(_, _)
            | TokenType::VersionDirective(_, _) => {
                return Err(ConfigError::UnsupportedYamlFeature { feature: "tag" });
            }
            TokenType::Key => {
                if yaml_source_character(&source_characters, token.0) == Some('?') {
                    return Err(ConfigError::UnsupportedYamlFeature {
                        feature: "explicit mapping key",
                    });
                }
                mapping_key_scalar = true;
            }
            TokenType::Value => mapping_key_scalar = false,
            TokenType::Scalar(TScalarStyle::Plain, scalar) => {
                if mapping_key_scalar && scalar == "<<" {
                    return Err(ConfigError::UnsupportedYamlFeature {
                        feature: "merge key",
                    });
                }
                mapping_key_scalar = false;
                reject_noncanonical_yaml_scalar(&scalar)?;
            }
            TokenType::Scalar(_, _) => mapping_key_scalar = false,
            _ => mapping_key_scalar = false,
        }
    }

    if let Some(error) = scanner.get_error() {
        return Err(ConfigError::InvalidDocument {
            reason: error.to_string(),
        });
    }

    Ok(())
}

fn yaml_source_character(source: &[char], marker: yaml_rust2::scanner::Marker) -> Option<char> {
    source.get(marker.index()).copied()
}

fn reject_noncanonical_yaml_scalar(value: &str) -> Result<(), ConfigError> {
    match Yaml::from_str(value) {
        Yaml::Integer(_) | Yaml::Real(_) if !is_canonical_i64(value) => {
            Err(ConfigError::NonCanonicalNumber)
        }
        _ if is_noncanonical_yaml_numeric_string(value) => Err(ConfigError::NonCanonicalNumber),
        _ => Ok(()),
    }
}

fn is_canonical_i64(value: &str) -> bool {
    let digits = value.strip_prefix('-').unwrap_or(value);
    !digits.is_empty()
        && (digits == "0"
            || (digits.as_bytes().first().is_some_and(u8::is_ascii_digit)
                && digits.as_bytes().first() != Some(&b'0')
                && digits.bytes().all(|byte| byte.is_ascii_digit())))
        && value != "-0"
        && value.parse::<i64>().is_ok()
}

fn is_noncanonical_yaml_numeric_string(value: &str) -> bool {
    let unsigned = value
        .strip_prefix('+')
        .or_else(|| value.strip_prefix('-'))
        .unwrap_or(value);

    is_radix_literal(unsigned, "0x", |byte| byte.is_ascii_hexdigit())
        || is_radix_literal(unsigned, "0X", |byte| byte.is_ascii_hexdigit())
        || is_radix_literal(unsigned, "0o", |byte| matches!(byte, b'0'..=b'7'))
        || is_radix_literal(unsigned, "0O", |byte| matches!(byte, b'0'..=b'7'))
        || is_radix_literal(unsigned, "0b", |byte| matches!(byte, b'0' | b'1'))
        || is_radix_literal(unsigned, "0B", |byte| matches!(byte, b'0' | b'1'))
        || is_underscored_decimal(value, unsigned)
}

fn is_radix_literal(value: &str, prefix: &str, is_digit: impl Fn(u8) -> bool) -> bool {
    let Some(digits) = value.strip_prefix(prefix) else {
        return false;
    };
    !digits.is_empty()
        && digits.bytes().all(|byte| byte == b'_' || is_digit(byte))
        && digits.bytes().any(|byte| byte != b'_')
}

fn is_underscored_decimal(value: &str, unsigned: &str) -> bool {
    if !value.contains('_') {
        return false;
    }

    let mut exponent_separator = None;
    for (index, byte) in unsigned.bytes().enumerate() {
        if matches!(byte, b'e' | b'E') && exponent_separator.replace(index).is_some() {
            return false;
        }
    }
    let (mantissa, exponent) = exponent_separator.map_or((unsigned, None), |index| {
        (&unsigned[..index], Some(&unsigned[index + 1..]))
    });

    let mut mantissa_parts = mantissa.split('.');
    let integer = mantissa_parts.next().unwrap_or_default();
    let fraction = mantissa_parts.next();
    if mantissa_parts.next().is_some() {
        return false;
    }
    let mantissa_valid = match fraction {
        Some(fraction) => {
            (integer.is_empty() || is_underscored_digit_component(integer))
                && is_underscored_digit_component(fraction)
        }
        None => is_underscored_digit_component(integer),
    };
    let exponent_valid = exponent.is_none_or(|exponent| {
        let digits = exponent
            .strip_prefix('+')
            .or_else(|| exponent.strip_prefix('-'))
            .unwrap_or(exponent);
        is_underscored_digit_component(digits)
    });

    mantissa_valid && exponent_valid
}

fn is_underscored_digit_component(value: &str) -> bool {
    let bytes = value.as_bytes();
    !bytes.is_empty()
        && bytes.first().is_some_and(u8::is_ascii_digit)
        && bytes.last().is_some_and(u8::is_ascii_digit)
        && bytes.iter().enumerate().all(|(index, byte)| {
            byte.is_ascii_digit()
                || (*byte == b'_'
                    && index > 0
                    && index + 1 < bytes.len()
                    && bytes[index - 1].is_ascii_digit()
                    && bytes[index + 1].is_ascii_digit())
        })
}
fn json_string_quote_is_escaped(bytes: &[u8], index: usize) -> bool {
    let mut count = 0usize;
    let mut cursor = index;
    while cursor > 0 && bytes[cursor - 1] == b'\\' {
        count += 1;
        cursor -= 1;
    }
    count % 2 == 1
}
fn count_node(
    depth: usize,
    nodes: &mut usize,
    limits: ProcedureParseLimitsV1,
) -> Result<(), ConfigError> {
    if depth > limits.max_depth {
        return Err(ConfigError::InputTooDeep {
            maximum: limits.max_depth,
            actual: depth,
        });
    }
    *nodes += 1;
    if *nodes > limits.max_nodes {
        return Err(ConfigError::InputTooComplex {
            maximum: limits.max_nodes,
            actual: *nodes,
        });
    }
    Ok(())
}

struct JsonParser<'a> {
    input: &'a str,
    position: usize,
    nodes: usize,
    limits: ProcedureParseLimitsV1,
}

impl<'a> JsonParser<'a> {
    fn new(input: &'a str, limits: ProcedureParseLimitsV1) -> Self {
        Self {
            input,
            position: 0,
            nodes: 0,
            limits,
        }
    }

    fn parse(mut self) -> Result<Value, ConfigError> {
        self.skip_whitespace();
        let value = self.parse_value(1)?;
        self.skip_whitespace();
        if self.position != self.input.len() {
            return Err(self.invalid("trailing data after JSON document"));
        }
        Ok(value)
    }

    fn parse_value(&mut self, depth: usize) -> Result<Value, ConfigError> {
        self.count_node(depth)?;
        match self.peek() {
            Some(b'{') => self.parse_object(depth),
            Some(b'[') => self.parse_array(depth),
            Some(b'"') => Ok(Value::String(self.parse_string()?)),
            Some(b't') => {
                self.expect_literal("true")?;
                Ok(Value::Bool(true))
            }
            Some(b'f') => {
                self.expect_literal("false")?;
                Ok(Value::Bool(false))
            }
            Some(b'n') => {
                self.expect_literal("null")?;
                Ok(Value::Null)
            }
            Some(b'-' | b'0'..=b'9') => self.parse_number(),
            _ => Err(self.invalid("expected a JSON value")),
        }
    }

    fn parse_object(&mut self, depth: usize) -> Result<Value, ConfigError> {
        self.position += 1;
        self.skip_whitespace();
        let mut values = Map::new();
        let mut keys = BTreeSet::new();
        if self.consume(b'}') {
            return Ok(Value::Object(values));
        }
        loop {
            if self.peek() != Some(b'"') {
                return Err(self.invalid("JSON object keys must be strings"));
            }
            let key = self.parse_string()?;
            if !keys.insert(key.clone()) {
                return Err(ConfigError::DuplicateKey { key });
            }
            self.skip_whitespace();
            self.expect_byte(b':')?;
            self.skip_whitespace();
            let value = self.parse_value(depth + 1)?;
            values.insert(key, value);
            self.skip_whitespace();
            if self.consume(b'}') {
                return Ok(Value::Object(values));
            }
            self.expect_byte(b',')?;
            self.skip_whitespace();
        }
    }

    fn parse_array(&mut self, depth: usize) -> Result<Value, ConfigError> {
        self.position += 1;
        self.skip_whitespace();
        let mut values = Vec::new();
        if self.consume(b']') {
            return Ok(Value::Array(values));
        }
        loop {
            values.push(self.parse_value(depth + 1)?);
            self.skip_whitespace();
            if self.consume(b']') {
                return Ok(Value::Array(values));
            }
            self.expect_byte(b',')?;
            self.skip_whitespace();
        }
    }

    fn parse_number(&mut self) -> Result<Value, ConfigError> {
        let start = self.position;
        if self.consume(b'-') && self.peek().is_none() {
            return Err(self.invalid("incomplete JSON number"));
        }
        match self.peek() {
            Some(b'0') => {
                self.position += 1;
                if self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
                    return Err(self.invalid("leading zero in JSON number"));
                }
            }
            Some(b'1'..=b'9') => {
                self.position += 1;
                while self.peek().is_some_and(|byte| byte.is_ascii_digit()) {
                    self.position += 1;
                }
            }
            _ => return Err(self.invalid("invalid JSON number")),
        }
        if self
            .peek()
            .is_some_and(|byte| matches!(byte, b'.' | b'e' | b'E'))
        {
            return Err(ConfigError::NonCanonicalNumber);
        }
        if self
            .peek()
            .is_some_and(|byte| !matches!(byte, b',' | b']' | b'}' | b' ' | b'\n' | b'\r' | b'\t'))
        {
            return Err(self.invalid("invalid JSON number"));
        }
        let number = &self.input[start..self.position];
        if number == "-0" {
            return Err(ConfigError::NonCanonicalNumber);
        }
        if let Ok(value) = number.parse::<i64>() {
            return Ok(Value::Number(Number::from(value)));
        }
        if let (false, Ok(value)) = (number.starts_with('-'), number.parse::<u64>()) {
            return Ok(Value::Number(Number::from(value)));
        }
        Err(ConfigError::NonCanonicalNumber)
    }

    fn parse_string(&mut self) -> Result<String, ConfigError> {
        let start = self.position;
        self.position += 1;
        let bytes = self.input.as_bytes();
        while self.position < bytes.len() {
            match bytes[self.position] {
                b'"' if !json_string_quote_is_escaped(bytes, self.position) => {
                    self.position += 1;
                    return serde_json::from_str(&self.input[start..self.position]).map_err(
                        |error| ConfigError::InvalidDocument {
                            reason: error.to_string(),
                        },
                    );
                }
                byte if byte < 0x20 => return Err(self.invalid("control character in JSON string")),
                _ => self.position += 1,
            }
        }
        Err(self.invalid("unterminated JSON string"))
    }

    fn expect_literal(&mut self, literal: &str) -> Result<(), ConfigError> {
        if self.input[self.position..].starts_with(literal) {
            self.position += literal.len();
            if self
                .peek()
                .is_some_and(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
            {
                return Err(self.invalid("invalid JSON literal"));
            }
            Ok(())
        } else {
            Err(self.invalid("invalid JSON literal"))
        }
    }

    fn expect_byte(&mut self, expected: u8) -> Result<(), ConfigError> {
        if self.consume(expected) {
            Ok(())
        } else {
            Err(self.invalid("invalid JSON syntax"))
        }
    }

    fn consume(&mut self, expected: u8) -> bool {
        if self.peek() == Some(expected) {
            self.position += 1;
            true
        } else {
            false
        }
    }

    fn skip_whitespace(&mut self) {
        while self
            .peek()
            .is_some_and(|byte| matches!(byte, b' ' | b'\n' | b'\r' | b'\t'))
        {
            self.position += 1;
        }
    }

    fn peek(&self) -> Option<u8> {
        self.input.as_bytes().get(self.position).copied()
    }

    fn count_node(&mut self, depth: usize) -> Result<(), ConfigError> {
        count_node(depth, &mut self.nodes, self.limits)
    }

    fn invalid(&self, reason: &str) -> ConfigError {
        ConfigError::InvalidDocument {
            reason: reason.to_owned(),
        }
    }
}
