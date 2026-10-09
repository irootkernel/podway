//! The public failure boundary validates substantive domain details and recovery identity.

use podway_core::{
    AttemptId, CriterionId, DomainError, GoalRevisionNumberV2, GraphNodeId, ItemId, Revision,
    SessionId, SessionLifecycle, Sha256Digest,
};
use podway_engine::{
    direct_error::{DirectErrorContextV1, render_direct_error_v1},
    direct_execution::{DirectDelegationFailureV1, DirectExecutionErrorV1},
    execution::ProcedureV2StartPreparationErrorV1,
};
use podway_protocol::{
    CommandNameV1, RequestIdV1, Rfc3339MillisV1, validate_daemon_free_document_v1,
};
use podway_store::{
    GraphMutationErrorV2, IdempotencyKeyV1, StoreErrorV1, direct_store::DirectStoreErrorV1,
};

const ID: &str = "00000000-0000-4000-8000-000000000001";
const OTHER: &str = "00000000-0000-4000-8000-000000000002";

fn context(command: &str) -> DirectErrorContextV1 {
    DirectErrorContextV1 {
        request_id: RequestIdV1::new(ID).unwrap(),
        command: CommandNameV1::new(command).unwrap(),
        observed_at: Rfc3339MillisV1::new("2023-11-14T22:13:20.000Z").unwrap(),
        idempotency_key: Some(IdempotencyKeyV1::new("original-key").unwrap()),
        timeout_ms: 5000,
        maintenance: None,
    }
}

fn assert_error(error: DirectExecutionErrorV1, command: &str, code: &str) -> serde_json::Value {
    let output = render_direct_error_v1(&error, &context(command))
        .unwrap_or_else(|failure| panic!("{code} ({error:?}) did not validate: {failure:?}"));
    let value = output.into_value();
    assert_eq!(value["code"], code);
    assert!(validate_daemon_free_document_v1("podway.error/v2", &value));
    if let Some(recovery) = value["details"].get("recovery") {
        assert!(validate_daemon_free_document_v1(
            "podway.recovery-recipe/v2",
            recovery
        ));
    }
    assert!(value.get("job").is_none());
    assert!(value["details"].get("admission").is_none());
    value
}

#[test]
fn direct_contention_errors_identify_the_actual_admission_boundary() {
    for (error, command, boundary) in [
        (DirectStoreErrorV1::Busy, "item.set", "sqlite_write_lock"),
        (
            DirectStoreErrorV1::ConnectionBusy,
            "session.status",
            "connection_gate",
        ),
        (
            DirectStoreErrorV1::ReadAdmissionBusy,
            "session.status",
            "sqlite_read_admission",
        ),
    ] {
        let value = assert_error(
            DirectExecutionErrorV1::Store(error),
            command,
            "WORKSPACE_BUSY",
        );
        assert_eq!(value["details"]["boundary"], boundary);
        assert_eq!(value["details"]["timeout_ms"], 5000);
        assert_eq!(value["retryable"], true);
        assert_eq!(value["exit_code"], 4);
    }
}

#[test]
fn actual_connection_gate_contention_returns_a_valid_public_error() {
    use std::{
        os::unix::fs::MetadataExt,
        sync::{Arc, Mutex, mpsc},
        time::{Duration, Instant},
    };

    use podway_engine::{
        direct_api::{DirectCommandV1, DirectRequestV1, DirectResponseV1, execute_direct_v1},
        direct_execution::NativeDirectIdSourceV1,
        workspace::DirectWorkspaceResolverV1,
    };
    use podway_git::NativeGitResolverV1;
    use podway_store::{
        CanonicalExecutionJsonV1,
        direct_store::{
            DirectMutationRequestV1, DirectOutcomeScopeV1, DirectTransactionBoundaryV1,
        },
    };

    use super::int_v2dfc006_direct_execution::{Clock, deadline, digest, fixture};

    let (_root, selector, store, output) = fixture();
    let (ready_tx, ready_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let release = Arc::new(Mutex::new(release_rx));
    let writer = store.clone().with_boundary_observer(move |boundary| {
        if boundary == DirectTransactionBoundaryV1::AfterCommitBeforeOutput {
            ready_tx.send(()).unwrap();
            release
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5))
                .unwrap();
        }
    });
    let child = std::thread::spawn(move || {
        let request = DirectMutationRequestV1::new(
            IdempotencyKeyV1::new("gate-writer").unwrap(),
            digest(),
            "workspace.init",
            output.as_value()["request_id"].as_str().unwrap(),
            DirectOutcomeScopeV1::Workspace,
        )
        .unwrap();
        writer.mutate(
            &request,
            deadline(),
            || podway_core::UnixMillis::new(1_700_000_000_001),
            |transaction| {
                transaction.now();
                Ok(CanonicalExecutionJsonV1::new(
                    podway_core::canonicalize_json_v1(output.as_value()).unwrap(),
                )
                .unwrap())
            },
        )
    });
    if let Err(error) = ready_rx.recv_timeout(Duration::from_secs(5)) {
        panic!(
            "writer never reached the held connection: {error}; {:?}",
            child.join().unwrap()
        );
    }
    let images = || {
        let mut files: Vec<_> = std::fs::read_dir(store.layout().runtime_directory())
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                let metadata = entry.metadata().unwrap();
                (
                    entry.file_name(),
                    metadata.dev(),
                    metadata.ino(),
                    std::fs::read(entry.path()).unwrap(),
                )
            })
            .collect();
        files.sort();
        files
    };
    let before = images();
    let resolver = DirectWorkspaceResolverV1::new(NativeGitResolverV1::new(), digest());
    let responses = [0_u64, 20].map(|timeout| {
        let started = Instant::now();
        let response = execute_direct_v1(
            &resolver,
            DirectRequestV1 {
                request_id: RequestIdV1::new(ID).unwrap(),
                selector: selector.clone(),
                expected_workspace_id: None,
                command: DirectCommandV1::WorkspaceShow,
                idempotency_key: None,
                deadline: started + Duration::from_millis(timeout),
                observed_at: Rfc3339MillisV1::new("2023-11-14T22:13:20.001Z").unwrap(),
                timeout_ms: timeout,
            },
            &Clock,
            &NativeDirectIdSourceV1,
        );
        (timeout, response, started.elapsed())
    });
    let after = images();
    release_tx.send(()).unwrap();
    assert!(child.join().unwrap().is_ok());
    assert_eq!(after, before);
    for (timeout, response, elapsed) in responses {
        let DirectResponseV1::Failure(error) = response.unwrap() else {
            panic!("read admitted while the actual writer still owns its connection");
        };
        let value = error.into_value();
        assert!(validate_daemon_free_document_v1("podway.error/v2", &value));
        assert_eq!(value["command"], "workspace.show");
        assert_eq!(value["code"], "WORKSPACE_BUSY");
        assert_eq!(value["details"]["boundary"], "connection_gate");
        assert_eq!(value["details"]["timeout_ms"], timeout);
        assert_eq!(value["exit_code"], 4);
        assert_eq!(value["retryable"], true);
        assert!(elapsed < Duration::from_secs(2));
    }
    assert_eq!(
        store
            .read(deadline(), |snapshot| snapshot.committed_sequence())
            .unwrap(),
        2
    );
}

#[test]
fn direct_failure_catalog_preserves_store_fences_domain_and_recovery() {
    let session = SessionId::new(ID).unwrap();
    let attempt = AttemptId::new(ID).unwrap();
    let sha = Sha256Digest::new(format!("sha256:{}", "a".repeat(64))).unwrap();
    let graph = |error| DirectExecutionErrorV1::Store(DirectStoreErrorV1::Domain(error));
    let store = |error| DirectExecutionErrorV1::Store(error);
    let cases = vec![
        (
            store(DirectStoreErrorV1::NotInitialized),
            "WORKSPACE_NOT_INITIALIZED",
        ),
        (store(DirectStoreErrorV1::Busy), "WORKSPACE_BUSY"),
        (store(DirectStoreErrorV1::ConnectionBusy), "WORKSPACE_BUSY"),
        (
            store(DirectStoreErrorV1::ReadAdmissionBusy),
            "WORKSPACE_BUSY",
        ),
        (
            store(DirectStoreErrorV1::Maintenance),
            "WORKSPACE_MAINTENANCE",
        ),
        (
            store(DirectStoreErrorV1::Legacy {
                generation: Some(13),
                marker: Some("reset.marker"),
            }),
            "LEGACY_RUNTIME_STATE_UNSUPPORTED",
        ),
        (
            store(DirectStoreErrorV1::WriterMismatch {
                expected: sha.clone(),
                actual: sha.as_str().into(),
            }),
            "WORKSPACE_CONTRACT_MISMATCH",
        ),
        (
            store(DirectStoreErrorV1::UnsupportedGeneration { generation: 15 }),
            "WORKSPACE_SCHEMA_UNSUPPORTED",
        ),
        (
            store(DirectStoreErrorV1::Unreadable),
            "WORKSPACE_STATE_UNREADABLE",
        ),
        (
            store(DirectStoreErrorV1::SchemaMismatch),
            "WORKSPACE_STATE_UNREADABLE",
        ),
        (
            store(DirectStoreErrorV1::BindingMismatch),
            "WORKSPACE_ID_CONFLICT",
        ),
        (
            store(DirectStoreErrorV1::IdempotencyKeyReused),
            "IDEMPOTENCY_KEY_REUSED",
        ),
        (
            store(DirectStoreErrorV1::MutationOutcomeUnknown),
            "MUTATION_OUTCOME_UNKNOWN",
        ),
        (
            graph(GraphMutationErrorV2::SessionRevisionConflict {
                expected: Revision::new(2),
                actual: Revision::new(3),
            }),
            "SESSION_REVISION_CONFLICT",
        ),
        (
            graph(GraphMutationErrorV2::AttemptNotCurrent {
                expected: attempt.clone(),
                actual: None,
            }),
            "ATTEMPT_NOT_CURRENT",
        ),
        (
            graph(GraphMutationErrorV2::Domain(
                DomainError::SessionIdentityMismatch {
                    expected: session.clone(),
                    actual: None,
                },
            )),
            "SESSION_ID_MISMATCH",
        ),
        (
            graph(GraphMutationErrorV2::Domain(
                DomainError::BlockerLimitReached {
                    maximum_open_blockers: 64,
                },
            )),
            "BLOCKER_LIMIT_REACHED",
        ),
        (
            graph(GraphMutationErrorV2::Domain(DomainError::ArtifactChanged)),
            "ARTIFACT_CHANGED",
        ),
        (
            graph(GraphMutationErrorV2::ItemTypeMismatch),
            "ITEM_TYPE_MISMATCH",
        ),
        (
            graph(GraphMutationErrorV2::ItemConstraintFailed),
            "ITEM_CONSTRAINT_FAILED",
        ),
        (
            graph(GraphMutationErrorV2::GoalTrackingNotEnabled),
            "GOAL_TRACKING_NOT_ENABLED",
        ),
        (
            graph(GraphMutationErrorV2::SessionGoalAlreadyDefined {
                goal_revision: GoalRevisionNumberV2::new(1),
            }),
            "SESSION_GOAL_ALREADY_DEFINED",
        ),
        (
            graph(GraphMutationErrorV2::GoalRevisionStale {
                expected: GoalRevisionNumberV2::new(1),
                actual: GoalRevisionNumberV2::new(2),
            }),
            "GOAL_REVISION_STALE",
        ),
        (
            graph(GraphMutationErrorV2::GoalRevisionTargetNotAllowed {
                target_graph_node_id: GraphNodeId::new("work").unwrap(),
            }),
            "GOAL_REVISION_TARGET_NOT_ALLOWED",
        ),
        (
            graph(GraphMutationErrorV2::GoalRevisionTargetNotRevisionSafe {
                target_graph_node_id: GraphNodeId::new("work").unwrap(),
            }),
            "GOAL_REVISION_TARGET_NOT_REVISION_SAFE",
        ),
        (
            graph(GraphMutationErrorV2::ReactivationFlagRequired),
            "REACTIVATION_FLAG_REQUIRED",
        ),
        (
            graph(GraphMutationErrorV2::CriterionNotFound {
                criterion_id: CriterionId::new("verified").unwrap(),
            }),
            "CRITERION_NOT_FOUND",
        ),
        (
            graph(GraphMutationErrorV2::CriterionCitationInvalid {
                criterion_id: CriterionId::new("verified").unwrap(),
                citation: podway_core::CriterionCitationV2::Item(ItemId::new("report").unwrap()),
            }),
            "CRITERION_CITATION_INVALID",
        ),
        (
            graph(GraphMutationErrorV2::CriterionResultMissing {
                missing_criterion_ids: vec![CriterionId::new("verified").unwrap()],
            }),
            "CRITERION_RESULT_MISSING",
        ),
        (
            graph(GraphMutationErrorV2::FreshGoalAssessmentMissing {
                goal_revision: GoalRevisionNumberV2::new(1),
            }),
            "FRESH_GOAL_ASSESSMENT_MISSING",
        ),
        (
            DirectExecutionErrorV1::Start(
                ProcedureV2StartPreparationErrorV1::DigestConfirmationRequired {
                    procedure_digest: sha.clone(),
                },
            ),
            "DIGEST_CONFIRMATION_REQUIRED",
        ),
        (
            DirectExecutionErrorV1::Start(
                ProcedureV2StartPreparationErrorV1::ProcedureDigestMismatch {
                    expected: sha.clone(),
                    actual: sha.clone(),
                },
            ),
            "PROCEDURE_DIGEST_MISMATCH",
        ),
        (
            DirectExecutionErrorV1::StartDecisionRequired {
                session_id: session,
                revision: Revision::new(1),
                lifecycle: SessionLifecycle::Running,
                current_terminal_disposition: false,
            },
            "SESSION_START_DECISION_REQUIRED",
        ),
        (
            DirectExecutionErrorV1::Delegation(DirectDelegationFailureV1::ReceiptRequired),
            "DELEGATION_RECEIPT_REQUIRED",
        ),
        (
            DirectExecutionErrorV1::Delegation(DirectDelegationFailureV1::ItemsChanged),
            "DELEGATION_RECEIPT_STALE",
        ),
        (
            DirectExecutionErrorV1::Domain(DomainError::RevisionOverflow {
                revision: Revision::new(u64::MAX),
            }),
            "INTERNAL_ERROR",
        ),
    ];
    for (error, code) in cases {
        assert_error(error, "session.complete", code);
    }
    for lifecycle in [
        SessionLifecycle::Running,
        SessionLifecycle::Completed,
        SessionLifecycle::Cancelled,
    ] {
        assert_error(
            store(DirectStoreErrorV1::State(
                StoreErrorV1::SessionResetNotEligibleV1 {
                    lifecycle,
                    current_terminal_disposition: false,
                },
            )),
            "session.reset",
            "SESSION_RESET_NOT_ELIGIBLE",
        );
    }
    for lifecycle in [
        SessionLifecycle::Prepared,
        SessionLifecycle::Running,
        SessionLifecycle::Completed,
        SessionLifecycle::Cancelled,
    ] {
        for disposed in [false, true] {
            assert_error(
                store(DirectStoreErrorV1::State(
                    StoreErrorV1::SessionStartStateConflictV1 {
                        lifecycle,
                        current_terminal_disposition: disposed,
                    },
                )),
                "session.start_replace",
                "SESSION_START_STATE_CONFLICT",
            );
        }
    }
    let value = assert_error(
        graph(GraphMutationErrorV2::ItemRevisionConflict {
            expected: Revision::new(2),
            actual: Revision::new(3),
        }),
        "item.set",
        "ITEM_REVISION_CONFLICT",
    );
    assert_eq!(value["details"]["expected_revision"], 2);
    assert_eq!(value["details"]["current_revision"], 3);
    let value = assert_error(
        DirectExecutionErrorV1::Domain(DomainError::SessionIdentityMismatch {
            expected: SessionId::new(ID).unwrap(),
            actual: Some(SessionId::new(OTHER).unwrap()),
        }),
        "session.begin",
        "SESSION_ID_MISMATCH",
    );
    assert_eq!(value["details"]["actual_session_id"], OTHER);
}
