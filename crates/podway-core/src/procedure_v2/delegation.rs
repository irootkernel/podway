//! Host-neutral declaration values for one delegated action placement.

use crate::{DelegationBindingRefV1, DomainError, Sha256Digest};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DelegationRoleRefV1(String);

impl DelegationRoleRefV1 {
    pub fn new(value: String) -> Result<Self, DomainError> {
        if value.is_empty() || value.len() > 256 || value.chars().any(char::is_control) {
            return Err(DomainError::InvalidState {
                reason: "delegation role reference must be one to 256 UTF-8 bytes without control characters",
            });
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DelegationAfterAcceptV1 {
    Retain,
    Release,
}

impl DelegationAfterAcceptV1 {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Retain => "retain",
            Self::Release => "release",
        }
    }
}

impl std::str::FromStr for DelegationAfterAcceptV1 {
    type Err = DomainError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "retain" => Ok(Self::Retain),
            "release" => Ok(Self::Release),
            _ => Err(DomainError::InvalidState {
                reason: "delegation after_accept must be retain or release",
            }),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DelegationDeclarationV1 {
    binding_ref: DelegationBindingRefV1,
    role_ref: DelegationRoleRefV1,
    role_snapshot_digest: Sha256Digest,
    max_rounds: u8,
    after_accept: DelegationAfterAcceptV1,
}

impl DelegationDeclarationV1 {
    pub fn new(
        binding_ref: DelegationBindingRefV1,
        role_ref: DelegationRoleRefV1,
        role_snapshot_digest: Sha256Digest,
        max_rounds: u8,
        after_accept: DelegationAfterAcceptV1,
    ) -> Result<Self, DomainError> {
        if !(1..=32).contains(&max_rounds) {
            return Err(DomainError::InvalidState {
                reason: "delegation max_rounds must be between one and 32",
            });
        }
        Ok(Self {
            binding_ref,
            role_ref,
            role_snapshot_digest,
            max_rounds,
            after_accept,
        })
    }

    pub fn binding_ref(&self) -> &DelegationBindingRefV1 {
        &self.binding_ref
    }

    pub fn role_ref(&self) -> &DelegationRoleRefV1 {
        &self.role_ref
    }

    pub fn role_snapshot_digest(&self) -> &Sha256Digest {
        &self.role_snapshot_digest
    }

    pub const fn max_rounds(&self) -> u8 {
        self.max_rounds
    }

    pub const fn after_accept(&self) -> DelegationAfterAcceptV1 {
        self.after_accept
    }
}
