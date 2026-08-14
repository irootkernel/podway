use crate::procedure::validate_media_type;
use crate::{DomainError, Sha256Digest};

/// The location mode of stored artifact metadata.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArtifactLocationKindV1 {
    LocalPath,
    ExternalReference,
}

/// Complete metadata for either a worktree-local artifact or an opaque external reference.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ArtifactValueV1 {
    location_kind: ArtifactLocationKindV1,
    location: String,
    digest: Sha256Digest,
    size_bytes: u64,
    media_type: String,
}

impl ArtifactValueV1 {
    pub fn local_path(
        path: impl Into<String>,
        digest: Sha256Digest,
        size_bytes: u64,
        media_type: impl Into<String>,
    ) -> Result<Self, DomainError> {
        let path = path.into();
        validate_safe_local_path(&path)?;
        Self::new(
            ArtifactLocationKindV1::LocalPath,
            path,
            digest,
            size_bytes,
            media_type.into(),
        )
    }

    pub fn external_reference(
        reference: impl Into<String>,
        digest: Sha256Digest,
        size_bytes: u64,
        media_type: impl Into<String>,
    ) -> Result<Self, DomainError> {
        let reference = reference.into();
        validate_external_reference(&reference)?;
        Self::new(
            ArtifactLocationKindV1::ExternalReference,
            reference,
            digest,
            size_bytes,
            media_type.into(),
        )
    }

    fn new(
        location_kind: ArtifactLocationKindV1,
        location: String,
        digest: Sha256Digest,
        size_bytes: u64,
        media_type: String,
    ) -> Result<Self, DomainError> {
        validate_media_type(&media_type)?;
        Ok(Self {
            location_kind,
            location,
            digest,
            size_bytes,
            media_type,
        })
    }

    pub const fn location_kind(&self) -> ArtifactLocationKindV1 {
        self.location_kind
    }

    pub fn location(&self) -> &str {
        &self.location
    }

    pub fn digest(&self) -> &Sha256Digest {
        &self.digest
    }

    pub const fn size_bytes(&self) -> u64 {
        self.size_bytes
    }

    pub fn media_type(&self) -> &str {
        &self.media_type
    }
}

fn validate_safe_local_path(path: &str) -> Result<(), DomainError> {
    if path.is_empty() || path.len() > 4_000 || path.starts_with('/') || path.contains('\\') {
        return Err(invalid(
            "artifact path must be a bounded relative POSIX path",
        ));
    }
    if path
        .split('/')
        .any(|component| component.is_empty() || component == "." || component == "..")
    {
        return Err(invalid("artifact path contains an unsafe component"));
    }
    if path.chars().any(char::is_control) {
        return Err(invalid("artifact path contains a control character"));
    }
    Ok(())
}

fn validate_external_reference(reference: &str) -> Result<(), DomainError> {
    if reference.trim().is_empty() || reference.len() > 4_000 {
        return Err(invalid("artifact reference must be non-empty and bounded"));
    }
    if reference.chars().any(char::is_control) {
        return Err(invalid("artifact reference contains a control character"));
    }
    Ok(())
}

const fn invalid(reason: &'static str) -> DomainError {
    DomainError::InvalidState { reason }
}
