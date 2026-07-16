//! Phase 0 daemon composition contracts.
//!
//! Requirement IDs: ARC-002, ARC-003, ARC-004, API-004.

use podway_core::{Sha256Digest, WorkspaceId};
use podway_daemon::{
    DaemonCompositionErrorV1, DaemonLimitsV1, RouteClassificationV1, SchedulerGenerationV1,
    SchedulerRetirementStateV1, WorkspaceSchedulerStateV1,
};
use podway_git::{DiagnosticPathDisplayV1, DurableWorktreeIdentityV1, LosslessPathV1};
use podway_protocol::OperationV1;

const WORKSPACE_ID: &str = "00000000-0000-0000-0000-000000000001";
const COMMON_DIRECTORY_DIGEST: &str =
    "sha256:0000000000000000000000000000000000000000000000000000000000000000";
const WORKTREE_ADMINISTRATION_DIGEST: &str =
    "sha256:1111111111111111111111111111111111111111111111111111111111111111";

fn durable_identity() -> DurableWorktreeIdentityV1 {
    DurableWorktreeIdentityV1::new(
        WorkspaceId::new(WORKSPACE_ID).expect("fixture workspace ID is canonical"),
        Sha256Digest::new(COMMON_DIRECTORY_DIGEST).expect("fixture digest is canonical"),
        Sha256Digest::new(WORKTREE_ADMINISTRATION_DIGEST).expect("fixture digest is canonical"),
        LosslessPathV1::from_raw_bytes(
            b"/tmp/podway-contract-worktree",
            DiagnosticPathDisplayV1::new("/tmp/podway-contract-worktree")
                .expect("fixture display is valid"),
        )
        .expect("fixture path is valid"),
    )
}

#[test]
fn arc_002_daemon_limits_reject_zero_and_preserve_positive_capacity() {
    assert_eq!(
        DaemonLimitsV1::new(0),
        Err(DaemonCompositionErrorV1::InvalidLimit {
            field: "max_concurrent_blocking_operations",
            value: 0,
        })
    );

    let limits = DaemonLimitsV1::new(3).expect("positive capacity is valid");
    assert_eq!(limits.max_concurrent_blocking_operations().get(), 3);
}

#[test]
fn api_004_operation_routes_are_exhaustive_and_admit_only_durable_operations() {
    let cases = [
        (OperationV1::Query, RouteClassificationV1::Read, false),
        (
            OperationV1::Mutate,
            RouteClassificationV1::MutationAdmission,
            true,
        ),
        (OperationV1::Control, RouteClassificationV1::Control, false),
        (
            OperationV1::Bootstrap,
            RouteClassificationV1::BootstrapAdmission,
            true,
        ),
    ];

    for (operation, expected_route, requires_durable_admission) in cases {
        let route = RouteClassificationV1::from_operation(operation);
        assert_eq!(route, expected_route, "unexpected route for {operation:?}");
        assert_eq!(
            route.requires_durable_admission(),
            requires_durable_admission,
            "unexpected durable-admission requirement for {operation:?}"
        );
    }
}

#[test]
fn arc_003_arc_004_scheduler_generations_and_retirement_states_are_explicit() {
    assert_eq!(SchedulerGenerationV1::initial().get(), 1);
    assert_eq!(
        SchedulerGenerationV1::new(0),
        Err(DaemonCompositionErrorV1::InvalidSchedulerGeneration { value: 0 })
    );

    let retiring_generation = SchedulerGenerationV1::new(41).expect("positive generation is valid");
    let replacement_generation = retiring_generation
        .next()
        .expect("non-maximum generation advances");
    assert_eq!(replacement_generation.get(), 42);
    assert!(replacement_generation > retiring_generation);

    let exhausted_generation = SchedulerGenerationV1::new(u64::MAX).expect("maximum is nonzero");
    assert_eq!(
        exhausted_generation.next(),
        Err(DaemonCompositionErrorV1::SchedulerGenerationExhausted {
            generation: exhausted_generation,
        })
    );

    for retirement in [
        SchedulerRetirementStateV1::Active,
        SchedulerRetirementStateV1::Retiring,
        SchedulerRetirementStateV1::Retired,
    ] {
        let scheduler =
            WorkspaceSchedulerStateV1::new(durable_identity(), retiring_generation, retirement);

        assert_eq!(scheduler.workspace_id().as_str(), WORKSPACE_ID);
        assert_eq!(
            scheduler.identity().common_directory_fingerprint().as_str(),
            COMMON_DIRECTORY_DIGEST
        );
        assert_eq!(
            scheduler
                .identity()
                .worktree_administration_fingerprint()
                .as_str(),
            WORKTREE_ADMINISTRATION_DIGEST
        );
        assert_eq!(scheduler.generation(), retiring_generation);
        assert_eq!(scheduler.retirement(), retirement);
    }
}
