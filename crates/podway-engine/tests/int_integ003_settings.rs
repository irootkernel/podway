//! Typed query rejects unsupported durable identity before native discovery.
use podway_core::{Sha256Digest, UnixMillis, WorkspaceId};
use podway_engine::{
    direct_api::{DirectCommandV1, DirectRequestV1, DirectResponseV1, execute_direct_v1},
    direct_execution::NativeDirectIdSourceV1,
    execution::ExecutionClockV1,
    workspace::DirectWorkspaceResolverV1,
};
use podway_git::{
    DiagnosticPathDisplayV1, LosslessPathV1, NativeGitResolverV1, WORKTREE_SELECTOR_VERSION_V1,
    WorktreeSelectorV1,
};
use podway_protocol::{ContractGenerationV1, RequestIdV1, Rfc3339MillisV1};
use podway_store::IdempotencyKeyV1;
use std::{
    os::unix::ffi::OsStrExt,
    time::{Duration, Instant},
};

struct NoClock;
impl ExecutionClockV1 for NoClock {
    fn now(&self) -> UnixMillis {
        panic!("settings query must use the observed request timestamp")
    }
}

#[test]
fn integ003_typed_query_rejects_keys_and_workspace_fences_before_git_or_home() {
    let root = tempfile::tempdir().unwrap();
    let selector = WorktreeSelectorV1::new(
        WORKTREE_SELECTOR_VERSION_V1,
        None,
        LosslessPathV1::from_raw_bytes(
            root.path().as_os_str().as_bytes(),
            DiagnosticPathDisplayV1::new("owned non-Git query fixture").unwrap(),
        )
        .unwrap(),
    )
    .unwrap();
    let resolver = DirectWorkspaceResolverV1::new(
        NativeGitResolverV1::new(),
        Sha256Digest::new(
            podway_protocol::build_identity_v1()
                .contract_manifest_digest()
                .to_owned(),
        )
        .unwrap(),
    );
    for (fence, key, code) in [
        (true, false, "REQUEST_INVALID"),
        (false, true, "REQUEST_INVALID"),
        (false, false, "NOT_A_GIT_WORKTREE"),
    ] {
        let response = execute_direct_v1(
            &resolver,
            DirectRequestV1 {
                request_id: RequestIdV1::new("00000000-0000-4000-8000-000000000001").unwrap(),
                selector: selector.clone(),
                expected_workspace_id: fence
                    .then(|| WorkspaceId::new("00000000-0000-4000-8000-000000000002").unwrap()),
                command: DirectCommandV1::IntegrationShow,
                idempotency_key: key.then(|| IdempotencyKeyV1::new("query-key").unwrap()),
                deadline: Instant::now() + Duration::from_secs(1),
                observed_at: Rfc3339MillisV1::new("2026-11-01T00:00:00.000Z").unwrap(),
                timeout_ms: 1000,
                generation: ContractGenerationV1::Generation1,
            },
            &NoClock,
            &NativeDirectIdSourceV1,
        )
        .unwrap();
        let DirectResponseV1::Failure(error) = response else {
            panic!("unexpected success")
        };
        assert_eq!(error.as_value()["code"], code);
        assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 0);
    }
}
