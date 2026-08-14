use sha2::{Digest as _, Sha256};

use crate::{DomainError, Sha256Digest, verify_canonical_json_v1};

/// Maximum number of open blockers carried in one v2 attempt projection.
pub const MAX_OPEN_BLOCKERS_PER_ATTEMPT_V2: usize = 64;

/// The persisted source category for an admitted procedure snapshot.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum ProcedureSourceKindV1 {
    Preset,
    File,
}

impl ProcedureSourceKindV1 {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Preset => "preset",
            Self::File => "file",
        }
    }

    pub fn from_row_value(value: &str) -> Result<Self, DomainError> {
        match value {
            "preset" => Ok(Self::Preset),
            "file" => Ok(Self::File),
            _ => Err(invalid("unknown procedure source kind")),
        }
    }
}

/// A normalized source label with its storage category and display form.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct ProcedureSourceLabelV1 {
    kind: ProcedureSourceKindV1,
    label: String,
    display: String,
}

impl ProcedureSourceLabelV1 {
    pub fn new(value: impl Into<String>) -> Result<Self, DomainError> {
        let value = value.into();
        if let Some(label) = value.strip_prefix("preset:") {
            return Self::preset(label);
        }
        if let Some(label) = value.strip_prefix("procedure:") {
            return Self::file(label);
        }
        Self::file(value)
    }

    pub fn preset(label: impl Into<String>) -> Result<Self, DomainError> {
        Self::from_parts(ProcedureSourceKindV1::Preset, label.into())
    }

    pub fn file(label: impl Into<String>) -> Result<Self, DomainError> {
        Self::from_parts(ProcedureSourceKindV1::File, label.into())
    }

    pub fn from_row(
        kind: ProcedureSourceKindV1,
        raw_label: impl Into<String>,
    ) -> Result<Self, DomainError> {
        Self::from_parts(kind, raw_label.into())
    }

    pub const fn kind(&self) -> ProcedureSourceKindV1 {
        self.kind
    }

    pub fn label(&self) -> &str {
        &self.label
    }

    pub fn display_label(&self) -> &str {
        &self.display
    }

    pub fn as_str(&self) -> &str {
        self.display_label()
    }

    pub fn into_inner(self) -> String {
        self.display
    }

    fn from_parts(kind: ProcedureSourceKindV1, label: String) -> Result<Self, DomainError> {
        if label.starts_with("preset:") || label.starts_with("procedure:") {
            return Err(invalid(
                "procedure source raw label must not contain a display prefix",
            ));
        }
        validate_text("procedure source label", &label, 1, 4_000, true)?;
        let display = match kind {
            ProcedureSourceKindV1::Preset => format!("preset:{label}"),
            ProcedureSourceKindV1::File => format!("procedure:{label}"),
        };
        validate_text("procedure source display label", &display, 1, 4_000, true)?;
        Ok(Self {
            kind,
            label,
            display,
        })
    }
}

/// Storage-bounded canonical Procedure v2 JSON supplied by configuration admission.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct CanonicalProcedureJsonV1(String);

impl CanonicalProcedureJsonV1 {
    pub fn new(value: impl Into<String>) -> Result<Self, DomainError> {
        let value = value.into();
        if value.is_empty() {
            return Err(invalid("canonical procedure JSON must not be empty"));
        }
        if value.len() > crate::MAX_PROCEDURE_DOCUMENT_BYTES {
            return Err(DomainError::ValueTooLong {
                field: "canonical procedure JSON",
                maximum: crate::MAX_PROCEDURE_DOCUMENT_BYTES,
                actual: value.len(),
            });
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

/// Verifies exact canonical Procedure v2 JSON bytes and their advertised digest.
pub fn verify_canonical_procedure_document_v2(
    canonical_json: &CanonicalProcedureJsonV1,
    digest: &Sha256Digest,
) -> Result<(), DomainError> {
    verify_canonical_json_v1(canonical_json.as_str().as_bytes())
        .map_err(|_| invalid("procedure JSON is not canonical"))?;
    let observed = Sha256Digest::new(format!(
        "sha256:{:x}",
        Sha256::digest(canonical_json.as_str().as_bytes())
    ))?;
    if &observed != digest {
        return Err(invalid(
            "procedure digest does not match canonical procedure JSON",
        ));
    }
    Ok(())
}

/// Procedure-independent item value discriminator used by Procedure v2 records.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ItemTypeV1 {
    Confirm,
    Text,
    Choice,
    Integer,
    List,
    Artifact,
}

pub(crate) fn validate_media_type(value: &str) -> Result<(), DomainError> {
    if value.len() > 255 {
        return Err(invalid("media type exceeds 255 bytes"));
    }
    let Some((kind, subtype)) = value.split_once('/') else {
        return Err(invalid("media type must contain exactly one slash"));
    };
    if kind.is_empty() || subtype.is_empty() || subtype.contains('/') {
        return Err(invalid("media type must contain a type and subtype"));
    }
    if !kind
        .as_bytes()
        .first()
        .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        || !subtype
            .as_bytes()
            .first()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        || !kind.bytes().all(is_media_token)
        || !subtype.bytes().all(is_media_token)
    {
        return Err(invalid(
            "media type must be lowercase ASCII without parameters",
        ));
    }
    Ok(())
}

fn is_media_token(byte: u8) -> bool {
    byte.is_ascii_lowercase()
        || byte.is_ascii_digit()
        || matches!(
            byte,
            b'!' | b'#' | b'$' | b'&' | b'^' | b'_' | b'.' | b'+' | b'-'
        )
}

pub(crate) fn validate_text(
    field: &'static str,
    value: &str,
    minimum: usize,
    maximum: usize,
    require_non_whitespace: bool,
) -> Result<(), DomainError> {
    let actual = value.chars().count();
    if actual < minimum || actual > maximum {
        return Err(invalid(field));
    }
    if require_non_whitespace && value.trim().is_empty() {
        return Err(invalid(field));
    }
    Ok(())
}

const fn invalid(reason: &'static str) -> DomainError {
    DomainError::InvalidState { reason }
}

#[cfg(test)]
mod tests {
    use super::CanonicalProcedureJsonV1;
    use crate::{DomainError, MAX_PROCEDURE_DOCUMENT_BYTES};

    #[test]
    fn canonical_procedure_json_enforces_empty_and_exact_byte_bounds() {
        assert_eq!(
            CanonicalProcedureJsonV1::new(""),
            Err(DomainError::InvalidState {
                reason: "canonical procedure JSON must not be empty",
            })
        );
        let exact = "x".repeat(MAX_PROCEDURE_DOCUMENT_BYTES);
        assert_eq!(
            CanonicalProcedureJsonV1::new(exact.clone())
                .unwrap()
                .as_str(),
            exact
        );
        assert!(
            CanonicalProcedureJsonV1::new("x".repeat(MAX_PROCEDURE_DOCUMENT_BYTES + 1)).is_err()
        );
    }
}
