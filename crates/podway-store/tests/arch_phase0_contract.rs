//! Phase 0B Store v1 construction and signature contracts.
//!
//! Requirements: STO-001 through STO-006, INV-S10, ARC-002, ARC-003.

use podway_core::{
    AttemptId, DomainCommand, DomainError, DomainResult, ItemId, JobId, Revision, SessionId,
    Sha256Digest, UnixMillis, WorkspaceId,
};
use podway_store::codec::PersistedTerminalResultV1;
use podway_store::{
    AdmitOutcomeV1, AdmitRequestV1, CancelOutcomeV1, CanonicalRequestDigestV1, ClaimTokenV1,
    ClaimedExecutionV1, ClaimedJobV1, DurableWorktreeIdentityV1, EpochMillisV1, IdempotencyKeyV1,
    JobIdV1, JobReceiptOrTerminalV1, JobReceiptV1, MAX_IDEMPOTENCY_KEY_BYTES_V1,
    MAX_WORKER_ID_BYTES_V1, PersistedGraphMutationFailureV2, PersistedTerminalReceiptV1,
    RevisionAttemptItemPreconditionsV1, RevisionV1, StateTransitionV1, StoreContractV1,
    StoreErrorV1, StoreIntegrityCheckV1, StoreInvariantV1, StoreRecordKindV1,
    StoreUnavailableReasonV1, StoreValueErrorV1, TerminalReceiptV1, TerminalResultV1, WorkerIdV1,
    WorkspaceViewV1,
};

fn digest(hex_digit: char) -> Sha256Digest {
    let value = format!("sha256:{}", hex_digit.to_string().repeat(64));
    match Sha256Digest::new(value) {
        Ok(value) => value,
        Err(_) => panic!("fixture digest must be valid"),
    }
}

fn job_id(value: &str) -> JobIdV1 {
    match JobId::new(value) {
        Ok(value) => value,
        Err(_) => panic!("fixture job ID must be valid"),
    }
}

fn workspace_uuid() -> WorkspaceId {
    match WorkspaceId::new("00000000-0000-4000-8000-000000000001") {
        Ok(value) => value,
        Err(_) => panic!("fixture workspace UUID must be valid"),
    }
}

fn identity() -> DurableWorktreeIdentityV1 {
    DurableWorktreeIdentityV1::new(digest('a'), workspace_uuid(), digest('b'))
}

fn receipt(sequence: u64, hex_digit: char) -> JobReceiptV1 {
    JobReceiptV1::new(
        sequence,
        job_id("00000000-0000-4000-8000-000000000003"),
        digest(hex_digit),
    )
}

fn failed_terminal_receipt(sequence: u64, hex_digit: char) -> TerminalReceiptV1 {
    TerminalReceiptV1::new(
        receipt(sequence, hex_digit),
        TerminalResultV1::Failure(DomainError::InvalidState {
            reason: "fixture failure",
        }),
    )
}

fn successful_terminal_receipt(sequence: u64, hex_digit: char) -> TerminalReceiptV1 {
    TerminalReceiptV1::new(
        receipt(sequence, hex_digit),
        TerminalResultV1::Success(DomainResult::WorkspaceInitialized {
            workspace_id: workspace_uuid(),
            revision: Revision::new(4),
        }),
    )
}

type ClaimNextSignature<T> = fn(
    &T,
    &DurableWorktreeIdentityV1,
    WorkerIdV1,
    EpochMillisV1,
) -> Result<Option<ClaimedJobV1>, StoreErrorV1>;

type CommitTerminalSignature<T> = fn(
    &T,
    ClaimTokenV1,
    RevisionV1,
    Option<StateTransitionV1>,
    TerminalResultV1,
    EpochMillisV1,
) -> Result<TerminalReceiptV1, StoreErrorV1>;

pub fn store_contract_signature_conforms<T: StoreContractV1>() {
    let _: fn(
        &T,
        &DurableWorktreeIdentityV1,
        AdmitRequestV1,
    ) -> Result<AdmitOutcomeV1, StoreErrorV1> = <T as StoreContractV1>::admit;
    let _: ClaimNextSignature<T> = <T as StoreContractV1>::claim_next;
    let _: fn(
        &T,
        &DurableWorktreeIdentityV1,
        JobIdV1,
        RevisionV1,
        EpochMillisV1,
    ) -> Result<CancelOutcomeV1, StoreErrorV1> = <T as StoreContractV1>::cancel_before_claim;
    let _: CommitTerminalSignature<T> = <T as StoreContractV1>::commit_terminal;
    let _: fn(&T, &DurableWorktreeIdentityV1) -> Result<WorkspaceViewV1, StoreErrorV1> =
        <T as StoreContractV1>::read_workspace_view;
}

#[test]
fn sto_003_store_v1_bounds_idempotency_and_worker_values() {
    let idempotency_key = match IdempotencyKeyV1::new("i".repeat(MAX_IDEMPOTENCY_KEY_BYTES_V1)) {
        Ok(value) => value,
        Err(_) => panic!("maximum-length idempotency key must be accepted"),
    };
    assert_eq!(idempotency_key.as_str().len(), MAX_IDEMPOTENCY_KEY_BYTES_V1);
    assert_eq!(MAX_IDEMPOTENCY_KEY_BYTES_V1, 256);
    let idempotency_key_at_utf8_boundary = format!("{}ab", "é".repeat(127));
    assert_eq!(idempotency_key_at_utf8_boundary.len(), 256);
    assert_eq!(
        idempotency_key_at_utf8_boundary.len(),
        MAX_IDEMPOTENCY_KEY_BYTES_V1
    );
    assert!(IdempotencyKeyV1::new(idempotency_key_at_utf8_boundary).is_ok());
    let idempotency_key_one_byte_over = format!("{}abc", "é".repeat(127));
    assert_eq!(idempotency_key_one_byte_over.len(), 257);
    assert_eq!(
        idempotency_key_one_byte_over.len(),
        MAX_IDEMPOTENCY_KEY_BYTES_V1 + 1
    );
    assert!(matches!(
        IdempotencyKeyV1::new(idempotency_key_one_byte_over),
        Err(StoreValueErrorV1::ValueTooLong {
            field: "idempotency key",
            maximum_bytes: MAX_IDEMPOTENCY_KEY_BYTES_V1,
        })
    ));
    assert!(matches!(
        IdempotencyKeyV1::new(""),
        Err(StoreValueErrorV1::EmptyValue {
            field: "idempotency key"
        })
    ));
    assert!(matches!(
        IdempotencyKeyV1::new("i".repeat(MAX_IDEMPOTENCY_KEY_BYTES_V1 + 1)),
        Err(StoreValueErrorV1::ValueTooLong {
            field: "idempotency key",
            maximum_bytes: MAX_IDEMPOTENCY_KEY_BYTES_V1,
        })
    ));

    let worker = match WorkerIdV1::new("w".repeat(MAX_WORKER_ID_BYTES_V1)) {
        Ok(value) => value,
        Err(_) => panic!("maximum-length worker identifier must be accepted"),
    };
    assert_eq!(worker.as_str().len(), MAX_WORKER_ID_BYTES_V1);
    assert_eq!(MAX_WORKER_ID_BYTES_V1, 128);
    let worker_at_utf8_boundary = format!("{}ab", "é".repeat(63));
    assert_eq!(worker_at_utf8_boundary.len(), 128);
    assert_eq!(worker_at_utf8_boundary.len(), MAX_WORKER_ID_BYTES_V1);
    assert!(WorkerIdV1::new(worker_at_utf8_boundary).is_ok());
    let worker_one_byte_over = format!("{}abc", "é".repeat(63));
    assert_eq!(worker_one_byte_over.len(), 129);
    assert_eq!(worker_one_byte_over.len(), MAX_WORKER_ID_BYTES_V1 + 1);
    assert!(matches!(
        WorkerIdV1::new(worker_one_byte_over),
        Err(StoreValueErrorV1::ValueTooLong {
            field: "worker identifier",
            maximum_bytes: MAX_WORKER_ID_BYTES_V1,
        })
    ));
    assert!(matches!(
        WorkerIdV1::new(""),
        Err(StoreValueErrorV1::EmptyValue {
            field: "worker identifier"
        })
    ));
    assert!(matches!(
        WorkerIdV1::new("w".repeat(MAX_WORKER_ID_BYTES_V1 + 1)),
        Err(StoreValueErrorV1::ValueTooLong {
            field: "worker identifier",
            maximum_bytes: MAX_WORKER_ID_BYTES_V1,
        })
    ));
}

#[test]
fn sto_003_sto_004_store_v1_binds_manifest_identity_and_admission_request() {
    let identity = identity();
    assert_eq!(
        identity.common_dir_identity().as_str(),
        digest('a').as_str()
    );
    assert_eq!(
        identity.workspace_uuid().as_str(),
        "00000000-0000-4000-8000-000000000001"
    );
    assert_eq!(
        identity.worktree_admin_identity().as_str(),
        digest('b').as_str()
    );

    let preconditions = match RevisionAttemptItemPreconditionsV1::new(
        Some(Revision::new(11)),
        Some(AttemptId::new("00000000-0000-4000-8000-000000000002").expect("valid attempt ID")),
        Some(ItemId::new("selected-item").expect("valid item ID")),
        Some(Revision::new(7)),
    ) {
        Ok(value) => value,
        Err(_) => panic!("item revision with an item ID must be accepted"),
    };
    assert_eq!(
        preconditions.expected_session_revision().map(Revision::get),
        Some(11)
    );
    assert_eq!(
        preconditions.expected_attempt_id().map(AttemptId::as_str),
        Some("00000000-0000-4000-8000-000000000002")
    );
    assert_eq!(
        preconditions.expected_item_id().map(ItemId::as_str),
        Some("selected-item")
    );
    assert_eq!(
        preconditions.expected_item_revision().map(Revision::get),
        Some(7)
    );

    let request = AdmitRequestV1::new(
        DomainCommand::ItemCheck {
            item_id: ItemId::new("active-item").expect("valid item ID"),
        },
        match IdempotencyKeyV1::new("stable-request-key") {
            Ok(value) => value,
            Err(_) => panic!("valid idempotency key"),
        },
        job_id("00000000-0000-4000-8000-000000000003"),
        preconditions,
        digest('c'),
        UnixMillis::new(1_234),
    );
    assert_eq!(
        request.job_id().as_str(),
        "00000000-0000-4000-8000-000000000003"
    );
    assert_eq!(request.idempotency_key().as_str(), "stable-request-key");
    assert_eq!(request.request_digest().as_str(), digest('c').as_str());
    assert!(matches!(
        request.command(),
        DomainCommand::ItemCheck { item_id } if item_id.as_str() == "active-item"
    ));
    assert_eq!(request.submitted_at().get(), 1_234);
    assert_eq!(
        request
            .preconditions()
            .expected_item_revision()
            .map(Revision::get),
        Some(7)
    );
}

#[test]
fn sto_001_sto_004_admission_and_cancellation_replay_every_receipt_variant() {
    let new = AdmitOutcomeV1::New(receipt(1, 'c'));
    assert!(matches!(
        new,
        AdmitOutcomeV1::New(job) if job.identity_sequence() == 1
    ));

    let existing_queued =
        AdmitOutcomeV1::Existing(JobReceiptOrTerminalV1::JobReceipt(receipt(2, 'd')));
    assert!(matches!(
        existing_queued,
        AdmitOutcomeV1::Existing(JobReceiptOrTerminalV1::JobReceipt(job))
            if job.identity_sequence() == 2
    ));

    let existing_terminal = AdmitOutcomeV1::Existing(JobReceiptOrTerminalV1::TerminalReceipt(
        PersistedTerminalReceiptV1::from_terminal_receipt(&successful_terminal_receipt(3, 'e')),
    ));
    assert!(matches!(
        existing_terminal,
        AdmitOutcomeV1::Existing(JobReceiptOrTerminalV1::TerminalReceipt(receipt))
            if matches!(receipt.result(), PersistedTerminalResultV1::Success(_))
    ));

    let cancelled = CancelOutcomeV1::Cancelled(receipt(4, 'f'));
    assert!(matches!(
        cancelled,
        CancelOutcomeV1::Cancelled(job) if job.identity_sequence() == 4
    ));

    let already_terminal_job =
        CancelOutcomeV1::AlreadyTerminal(JobReceiptOrTerminalV1::JobReceipt(receipt(5, 'a')));
    assert!(matches!(
        already_terminal_job,
        CancelOutcomeV1::AlreadyTerminal(JobReceiptOrTerminalV1::JobReceipt(job))
            if job.identity_sequence() == 5
    ));

    let already_terminal_result =
        CancelOutcomeV1::AlreadyTerminal(JobReceiptOrTerminalV1::TerminalReceipt(
            PersistedTerminalReceiptV1::from_terminal_receipt(&failed_terminal_receipt(6, 'b')),
        ));
    assert!(matches!(
        already_terminal_result,
        CancelOutcomeV1::AlreadyTerminal(JobReceiptOrTerminalV1::TerminalReceipt(receipt))
            if matches!(receipt.result(), PersistedTerminalResultV1::Failure(_))
    ));
}

#[test]
fn sto_002_sto_005_claim_and_terminal_construction_preserve_atomic_boundaries() {
    let claim = ClaimTokenV1::new(
        identity(),
        job_id("00000000-0000-4000-8000-000000000003"),
        Revision::new(9),
        WorkerIdV1::new("worker-1").expect("valid worker ID"),
    );
    assert_eq!(
        claim.identity().workspace_uuid().as_str(),
        workspace_uuid().as_str()
    );
    assert_eq!(claim.job_revision().get(), 9);
    assert_eq!(claim.worker().as_str(), "worker-1");

    let claimed = ClaimedJobV1::new_persisted(
        claim,
        receipt(7, 'c'),
        ClaimedExecutionV1::new(
            DomainCommand::WorkspaceInitialize,
            RevisionAttemptItemPreconditionsV1::new(None, None, None, None)
                .expect("empty preconditions must be valid"),
        ),
    );
    assert_eq!(claimed.job().identity_sequence(), 7);
    assert_eq!(
        claimed.claim().job_id().as_str(),
        "00000000-0000-4000-8000-000000000003"
    );
    assert_eq!(
        claimed.execution().command(),
        &DomainCommand::WorkspaceInitialize
    );
    let transition = StateTransitionV1::new_persisted(None, Revision::new(9), Revision::new(9))
        .expect("unchanged metadata transition");
    assert_eq!(transition.previous_workspace_revision().get(), 9);
    assert_eq!(transition.resulting_workspace_revision().get(), 9);
    assert!(matches!(
        StateTransitionV1::new_persisted(None, Revision::new(10), Revision::new(9),),
        Err(StoreValueErrorV1::SessionMutationRevisionMismatch)
    ));
    assert!(matches!(
        StateTransitionV1::new_persisted(None, Revision::new(9), Revision::new(10),),
        Err(StoreValueErrorV1::SessionMutationRevisionMismatch)
    ));

    let success =
        PersistedTerminalReceiptV1::from_terminal_receipt(&successful_terminal_receipt(8, 'd'));
    assert!(matches!(
        success.result(),
        PersistedTerminalResultV1::Success(_)
    ));
    assert_eq!(success.job().identity_sequence(), 8);

    let failure =
        PersistedTerminalReceiptV1::from_terminal_receipt(&failed_terminal_receipt(9, 'e'));
    assert!(matches!(
        failure.result(),
        PersistedTerminalResultV1::Failure(_)
    ));
    assert_eq!(failure.job().identity_sequence(), 9);

    let _: fn(JobReceiptV1, TerminalResultV1) -> TerminalReceiptV1 = TerminalReceiptV1::new;
}

#[test]
fn store_v1_constructs_every_typed_error_variant() {
    assert!(matches!(
        RevisionAttemptItemPreconditionsV1::new(None, None, None, Some(Revision::new(1))),
        Err(StoreValueErrorV1::ItemRevisionWithoutItem)
    ));

    let errors = vec![
        StoreErrorV1::AdmissionCommittedV1 {
            receipt: JobReceiptV1::new(
                1,
                job_id("00000000-0000-4000-8000-000000000003"),
                digest('c'),
            ),
            source: Box::new(StoreErrorV1::StorageUnavailableV1 {
                reason: StoreUnavailableReasonV1::Recovery,
            }),
        },
        StoreErrorV1::AdmissionOutcomeUnknownV1 {
            idempotency_key: IdempotencyKeyV1::new("unknown-admission").unwrap(),
        },
        StoreErrorV1::AlreadyClaimedV1 {
            job_id: job_id("00000000-0000-4000-8000-000000000003"),
        },
        StoreErrorV1::CancellationLostV1 {
            job_id: job_id("00000000-0000-4000-8000-000000000003"),
        },
        StoreErrorV1::ClaimStaleV1 {
            job_id: job_id("00000000-0000-4000-8000-000000000003"),
        },
        StoreErrorV1::CorruptStateV1 {
            record: StoreRecordKindV1::Job,
        },
        StoreErrorV1::IdempotencyDigestConflictV1 {
            expected: digest('d'),
            actual: digest('e'),
        },
        StoreErrorV1::InternalInvariantViolationV1 {
            invariant: StoreInvariantV1::MonotonicRevision,
        },
        StoreErrorV1::InvalidStateV1(StoreValueErrorV1::EmptyValue {
            field: "idempotency key",
        }),
        StoreErrorV1::PrimaryOperationAndCleanupFailureV1 {
            primary: Box::new(StoreErrorV1::StorageUnavailableV1 {
                reason: StoreUnavailableReasonV1::Busy,
            }),
            cleanup: Box::new(StoreErrorV1::StorageIntegrityV1 {
                check: StoreIntegrityCheckV1::ForeignKeys,
            }),
        },
        StoreErrorV1::JobNotFoundV1 {
            job_id: job_id("00000000-0000-4000-8000-000000000003"),
        },
        StoreErrorV1::NewerStateV1 {
            found_schema_version: 2,
            supported_schema_version: 1,
        },
        StoreErrorV1::PreconditionConflictV1 {
            expected: Some(Revision::new(4)),
            actual: Some(Revision::new(5)),
        },
        StoreErrorV1::ProcedureV2PreconditionFailedV1 {
            failure: PersistedGraphMutationFailureV2::SessionRevisionConflict {
                expected: Revision::new(4),
                actual: Revision::new(5),
            },
        },
        StoreErrorV1::SessionIdentityConflictV1 {
            expected: Some(SessionId::new("00000000-0000-4000-8000-000000000004").unwrap()),
            actual: Some(SessionId::new("00000000-0000-4000-8000-000000000005").unwrap()),
        },
        StoreErrorV1::StorageIntegrityV1 {
            check: StoreIntegrityCheckV1::JobQueue,
        },
        StoreErrorV1::StorageUnavailableV1 {
            reason: StoreUnavailableReasonV1::Locked,
        },
        StoreErrorV1::LegacyProcedureStateUnsupportedV1,
    ];

    for error in errors {
        match error {
            StoreErrorV1::AdmissionCommittedV1 { receipt, source } => {
                assert_eq!(receipt.identity_sequence(), 1);
                assert!(matches!(
                    *source,
                    StoreErrorV1::StorageUnavailableV1 {
                        reason: StoreUnavailableReasonV1::Recovery
                    }
                ));
            }
            StoreErrorV1::AdmissionOutcomeUnknownV1 { idempotency_key } => {
                assert_eq!(idempotency_key.as_str(), "unknown-admission");
            }
            StoreErrorV1::AlreadyClaimedV1 { job_id } => {
                assert_eq!(job_id.as_str(), "00000000-0000-4000-8000-000000000003");
            }
            StoreErrorV1::CancellationLostV1 { job_id } => {
                assert_eq!(job_id.as_str(), "00000000-0000-4000-8000-000000000003");
            }
            StoreErrorV1::ClaimStaleV1 { job_id } => {
                assert_eq!(job_id.as_str(), "00000000-0000-4000-8000-000000000003");
            }
            StoreErrorV1::CorruptStateV1 { record } => {
                assert_eq!(record, StoreRecordKindV1::Job);
            }
            StoreErrorV1::IdempotencyDigestConflictV1 { expected, actual } => {
                assert_eq!(expected, digest('d'));
                assert_eq!(actual, digest('e'));
            }
            StoreErrorV1::InternalInvariantViolationV1 { invariant } => {
                assert_eq!(invariant, StoreInvariantV1::MonotonicRevision);
            }
            StoreErrorV1::InvalidStateV1(value) => {
                assert_eq!(
                    value,
                    StoreValueErrorV1::EmptyValue {
                        field: "idempotency key",
                    }
                );
            }
            StoreErrorV1::PrimaryOperationAndCleanupFailureV1 { primary, cleanup } => {
                match (*primary, *cleanup) {
                    (
                        StoreErrorV1::StorageUnavailableV1 {
                            reason: StoreUnavailableReasonV1::Busy,
                        },
                        StoreErrorV1::StorageIntegrityV1 {
                            check: StoreIntegrityCheckV1::ForeignKeys,
                        },
                    ) => {}
                    (primary, cleanup) => {
                        panic!("unexpected primary {primary:?} and cleanup {cleanup:?}");
                    }
                }
            }
            StoreErrorV1::JobNotFoundV1 { job_id } => {
                assert_eq!(job_id.as_str(), "00000000-0000-4000-8000-000000000003");
            }
            StoreErrorV1::NewerStateV1 {
                found_schema_version,
                supported_schema_version,
            } => {
                assert_eq!(found_schema_version, 2);
                assert_eq!(supported_schema_version, 1);
            }
            StoreErrorV1::PreconditionConflictV1 { expected, actual } => {
                assert_eq!(expected, Some(Revision::new(4)));
                assert_eq!(actual, Some(Revision::new(5)));
            }
            StoreErrorV1::ProcedureV2PreconditionFailedV1 { failure } => {
                assert_eq!(
                    failure,
                    PersistedGraphMutationFailureV2::SessionRevisionConflict {
                        expected: Revision::new(4),
                        actual: Revision::new(5),
                    }
                );
            }
            StoreErrorV1::SessionIdentityConflictV1 { expected, actual } => {
                assert_eq!(
                    expected.unwrap().as_str(),
                    "00000000-0000-4000-8000-000000000004"
                );
                assert_eq!(
                    actual.unwrap().as_str(),
                    "00000000-0000-4000-8000-000000000005"
                );
            }
            StoreErrorV1::StorageIntegrityV1 { check } => {
                assert_eq!(check, StoreIntegrityCheckV1::JobQueue);
            }
            StoreErrorV1::StorageUnavailableV1 { reason } => {
                assert_eq!(reason, StoreUnavailableReasonV1::Locked);
            }
            StoreErrorV1::LegacyProcedureStateUnsupportedV1 => {}
            StoreErrorV1::SessionResetNotEligibleV1 { .. } => {}
            StoreErrorV1::TerminalDispositionAlreadyRecordedV1 { .. } => {}
        }
    }
}

#[test]
fn store_v1_boundary_aliases_are_core_values() {
    let _: CanonicalRequestDigestV1 = digest('a');
}
