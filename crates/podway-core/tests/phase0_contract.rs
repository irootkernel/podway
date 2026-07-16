use podway_core::{
    AttemptId, DomainError, ItemId, JobId, MAX_PROCEDURE_IDENTIFIER_BYTES, Revision, SessionId,
    Sha256Digest, StageId, WorkspaceId,
};

const CANONICAL_UUID: &str = "123e4567-e89b-12d3-a456-426614174000";

// ARC-007: Core contracts remain pure, deterministic, and independent of infrastructure.
#[test]
fn arc_007_uuid_newtypes_accept_canonical_values_and_reject_noncanonical_values() {
    assert_eq!(
        WorkspaceId::new(CANONICAL_UUID).unwrap().as_str(),
        CANONICAL_UUID
    );
    assert_eq!(
        SessionId::new(CANONICAL_UUID).unwrap().as_str(),
        CANONICAL_UUID
    );
    assert_eq!(
        AttemptId::new(CANONICAL_UUID).unwrap().as_str(),
        CANONICAL_UUID
    );
    assert_eq!(JobId::new(CANONICAL_UUID).unwrap().as_str(), CANONICAL_UUID);

    assert_eq!(
        WorkspaceId::new("123E4567-e89b-12d3-a456-426614174000"),
        Err(DomainError::InvalidUuid {
            field: "WorkspaceId",
        })
    );
    assert_eq!(
        SessionId::new("123e4567e89b-12d3-a456-426614174000"),
        Err(DomainError::InvalidUuid { field: "SessionId" })
    );
}

// DOM-001: Stage and item identifiers are stable lowercase kebab-case procedure keys.
#[test]
fn dom_001_stage_and_item_identifiers_enforce_the_frozen_contract() {
    let maximum_length_identifier = format!("a{}", "b".repeat(MAX_PROCEDURE_IDENTIFIER_BYTES - 1));

    assert_eq!(
        StageId::new("prepare-release").unwrap().as_str(),
        "prepare-release"
    );
    assert_eq!(
        ItemId::new(maximum_length_identifier.clone())
            .unwrap()
            .as_str(),
        maximum_length_identifier
    );
    assert_eq!(
        StageId::new("prepare--release"),
        Err(DomainError::InvalidIdentifier { field: "StageId" })
    );
    assert_eq!(
        ItemId::new(""),
        Err(DomainError::EmptyValue { field: "ItemId" })
    );
    assert_eq!(
        ItemId::new("A-valid-looking-item"),
        Err(DomainError::InvalidIdentifier { field: "ItemId" })
    );
}

// STO-004: Revisions support deterministic optimistic-concurrency increments and overflow failure.
#[test]
fn sto_004_revision_newtype_preserves_zero_increment_and_overflow_contracts() {
    assert_eq!(Revision::ZERO.get(), 0);
    assert_eq!(Revision::new(41).checked_next(), Ok(Revision::new(42)));
    assert_eq!(
        Revision::new(u64::MAX).checked_next(),
        Err(DomainError::RevisionOverflow {
            revision: Revision::new(u64::MAX),
        })
    );
}

// API-001: SHA-256 digests use one stable, serializable lowercase representation.
#[test]
fn api_001_sha256_digest_newtype_accepts_only_canonical_sha256_values() {
    let valid_digest = format!("sha256:{}", "a".repeat(64));

    assert_eq!(
        Sha256Digest::new(valid_digest.clone()).unwrap().as_str(),
        valid_digest
    );
    assert_eq!(
        Sha256Digest::new(format!("sha256:{}A", "a".repeat(63))),
        Err(DomainError::InvalidSha256Digest)
    );
    assert_eq!(
        Sha256Digest::new(format!("sha512:{}", "a".repeat(64))),
        Err(DomainError::InvalidSha256Digest)
    );
}
