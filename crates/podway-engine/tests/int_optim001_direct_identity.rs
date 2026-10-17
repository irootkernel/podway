//! Stored snapshot identity stays distinct from a revalidated wrapper.

use std::{
    os::unix::ffi::OsStrExt,
    process::Command,
    time::{Duration, Instant},
};

use podway_config::{
    ParsedProcedure, ProcedureDocumentFormat, parse_procedure_document, validate_procedure_v2,
};
use podway_core::{
    AttemptId, CanonicalProcedureJsonV1, ProcedureSnapshotId, ReasonV2, Revision, SessionId,
    Sha256Digest, TerminalDispositionV2, UnixMillis, WorkspaceId, canonicalize_json_v1,
};
use podway_engine::{
    direct_read_service::{
        project_direct_archive_show_v1, project_direct_delegation_package_v1,
        project_direct_next_v1, project_direct_observation_v1, project_direct_status_v1,
    },
    execution::{ExecutionErrorV1, required_local_artifacts_v2},
    v2_read_service::{
        GraphStatusTierV2, GraphViewErrorV2, delegation_work_package_from_view_v1,
        project_delegation_package_result_v1,
    },
};
use podway_git::{
    DiagnosticPathDisplayV1, GitResolverContractV1, LosslessPathV1, NativeGitResolverV1,
    WORKTREE_SELECTOR_VERSION_V1, WorkspaceLayoutInitializerV1, WorktreeSelectorV1,
};
use podway_store::{
    CanonicalExecutionJsonV1, DurableWorktreeIdentityV1, GraphSessionStateV2, GraphWorkspaceViewV2,
    IdempotencyKeyV1, ProcedureSnapshotV2, SessionArchiveSummaryV2, ValidatedWorkspaceRootV1,
    direct_store::{
        DirectMutationRequestV1, DirectOutcomeScopeV1, DirectStoreV1, DirectWorkspaceLayoutV1,
        DirectWorkspaceViewV1, ExclusiveMaintenanceLeaseV1,
    },
};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use tempfile::TempDir;

const DIGEST: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const WORKSPACE: &str = "00000000-0000-4000-8000-000000002101";
const SESSION: &str = "00000000-0000-4000-8000-000000002102";
const ATTEMPT: &str = "00000000-0000-4000-8000-000000002103";
const SNAPSHOT: &str = "00000000-0000-4000-8000-000000002106";
const INIT_REQUEST: &str = "00000000-0000-4000-8000-000000002107";
const SEED_REQUEST: &str = "00000000-0000-4000-8000-000000002108";
const ARCHIVE_REQUEST: &str = "00000000-0000-4000-8000-000000002109";
const NOW: u64 = 1_700_000_000_000;
const OBSERVED_AT: u64 = NOW + 30;

const LIST: &[u8] = br#"{"schema":"podway.procedure/v2","id":"optim-list","version":"1",
    "name":"Optim list","purpose":"Compare stored and revalidated list identity.",
    "node_definitions":{"work":{"type":"action","title":"Work","intent":"Record a list.",
        "items":[{"id":"findings","type":"list","prompt":"Record findings.","required":false}]}},
    "graph":{"entry":"work","nodes":[{"id":"work","use":"work","terminal":true}]}}"#;

fn delegated(source: &[u8]) -> Vec<u8> {
    let mut procedure: Value = serde_json::from_slice(source).unwrap();
    procedure["graph"]["nodes"][0]["delegation"] = json!({
        "binding_ref": "local-specialist",
        "role_ref": "reviewer",
        "role_snapshot_digest": DIGEST,
        "max_rounds": 3,
        "after_accept": "retain"
    });
    serde_json::to_vec(&procedure).unwrap()
}

fn deadline() -> Instant {
    Instant::now() + Duration::from_secs(10)
}

fn digest() -> Sha256Digest {
    Sha256Digest::new(DIGEST).unwrap()
}

fn attempt(id: &str) -> AttemptId {
    AttemptId::new(id).unwrap()
}

fn snapshot_of(source: &[u8]) -> ProcedureSnapshotV2 {
    podway_engine::execution::workspace_procedure_snapshot_from_bytes_v2(
        "procedure.json",
        source,
        ProcedureSnapshotId::new(SNAPSHOT).unwrap(),
        UnixMillis::new(NOW),
    )
    .unwrap()
}

fn prepared(source: &[u8]) -> GraphSessionStateV2 {
    GraphSessionStateV2::prepared(
        Revision::new(1),
        "Optim direct identity",
        snapshot_of(source),
        SessionId::new(SESSION).unwrap(),
        UnixMillis::new(NOW),
    )
    .unwrap()
}

fn running(source: &[u8]) -> GraphSessionStateV2 {
    prepared(source)
        .begin_v2(
            Revision::ZERO,
            attempt(ATTEMPT),
            None,
            UnixMillis::new(NOW + 1),
        )
        .unwrap()
        .into_state()
}

fn cancelled(source: &[u8]) -> GraphSessionStateV2 {
    let running = running(source);
    let active = running
        .trace()
        .active_attempt()
        .unwrap()
        .attempt_id()
        .clone();
    running
        .cancel_active_session_v2(
            running.trace().revision(),
            &active,
            ReasonV2::new("Cancel for projection identity.").unwrap(),
            UnixMillis::new(NOW + 2),
        )
        .unwrap()
        .into_state()
}

fn strip_materialized_list_bound(current: GraphSessionStateV2) -> GraphSessionStateV2 {
    let mut document: Value =
        serde_json::from_str(current.snapshot().canonical_json().as_str()).unwrap();
    let list = document["node_definitions"]
        .as_object_mut()
        .unwrap()
        .values_mut()
        .filter_map(|definition| definition.get_mut("items")?.as_array_mut())
        .flatten()
        .find(|item| item["type"] == "list")
        .unwrap()
        .as_object_mut()
        .unwrap();
    assert_eq!(list.remove("max_total_length"), Some(json!(1_000_000)));
    let canonical = canonicalize_json_v1(&document).unwrap();
    let digest =
        Sha256Digest::new(format!("sha256:{:x}", Sha256::digest(canonical.as_bytes()))).unwrap();
    assert_ne!(&digest, current.snapshot().digest());
    let snapshot = ProcedureSnapshotV2::new(
        current.snapshot().snapshot_id().clone(),
        CanonicalProcedureJsonV1::new(canonical).unwrap(),
        digest,
        current.snapshot().source().clone(),
        current.snapshot().created_at(),
    )
    .unwrap();
    GraphSessionStateV2::new_with_goal_state(
        current.workspace_revision(),
        current.task_title(),
        snapshot,
        current.trace().clone(),
        current.counters().to_vec(),
        current.attempt_metadata().to_vec(),
        current.workflow_memory().clone(),
        current.goal_state().clone(),
        current.created_at(),
        current.completed_at(),
        current.cancelled_at(),
        current.cancel_reason().map(str::to_owned),
    )
    .unwrap()
}

fn revalidated(state: &GraphSessionStateV2) -> podway_config::ValidatedProcedureV2 {
    let ParsedProcedure::V2(parsed) = parse_procedure_document(
        state.snapshot().canonical_json().as_str().as_bytes(),
        ProcedureDocumentFormat::Json,
    )
    .unwrap() else {
        panic!("stored snapshot must be Procedure v2");
    };
    validate_procedure_v2(parsed).unwrap()
}

fn provisioned_store() -> (TempDir, DirectStoreV1) {
    let fixture = tempfile::Builder::new()
        .prefix("optim001-direct-")
        .tempdir()
        .unwrap();
    assert!(
        Command::new("git")
            .args(["init", "--quiet"])
            .arg(fixture.path())
            .status()
            .unwrap()
            .success()
    );
    let path = std::fs::canonicalize(fixture.path()).unwrap();
    let selector = WorktreeSelectorV1::new(
        WORKTREE_SELECTOR_VERSION_V1,
        None,
        LosslessPathV1::from_raw_bytes(
            path.as_os_str().as_bytes(),
            DiagnosticPathDisplayV1::new(path.to_string_lossy()).unwrap(),
        )
        .unwrap(),
    )
    .unwrap();
    let worktree = NativeGitResolverV1::new().resolve(selector).unwrap();
    WorkspaceLayoutInitializerV1::new()
        .initialize(&worktree)
        .unwrap();
    let root =
        ValidatedWorkspaceRootV1::from_unix_bytes(path.as_os_str().as_bytes().to_vec()).unwrap();
    let store = DirectStoreV1::new(
        DirectWorkspaceLayoutV1::new(root).unwrap(),
        digest(),
        worktree.identity().common_directory_fingerprint().clone(),
        worktree
            .identity()
            .worktree_administration_fingerprint()
            .clone(),
    );
    let output = json!({
        "schema":"podway.output/v4", "request_id": INIT_REQUEST, "command":"workspace.init",
        "generated_at":"2023-11-14T22:13:20.000Z", "workspace": {
            "uuid": WORKSPACE, "root": path.to_string_lossy(), "latest_workspace_sequence": 1
        }, "result":{"schema":"podway.workspace-init-result/v2","initialized":true,"revision":0},
        "warnings":[]
    });
    let request = DirectMutationRequestV1::new(
        IdempotencyKeyV1::new("init-key").unwrap(),
        digest(),
        "workspace.init",
        INIT_REQUEST,
        DirectOutcomeScopeV1::Workspace,
    )
    .unwrap();
    let lease = ExclusiveMaintenanceLeaseV1::acquire(store.layout(), deadline(), true).unwrap();
    store
        .bootstrap(
            &lease,
            WorkspaceId::new(WORKSPACE).unwrap(),
            "optim-direct-identity",
            &request,
            deadline(),
            || UnixMillis::new(NOW),
            |_| Ok(CanonicalExecutionJsonV1::new(canonicalize_json_v1(&output).unwrap()).unwrap()),
        )
        .unwrap();
    drop(lease);
    (fixture, store)
}

fn mutate_outcome(command: &str, request_id: &str) -> CanonicalExecutionJsonV1 {
    CanonicalExecutionJsonV1::new(
        canonicalize_json_v1(&json!({
            "schema":"podway.output/v4",
            "request_id": request_id,
            "command": command,
            "generated_at":"2023-11-14T22:13:20.005Z",
            "result":{},
            "warnings":[]
        }))
        .unwrap(),
    )
    .unwrap()
}

fn seed_graph(store: &DirectStoreV1, graph: &GraphSessionStateV2, key: &str, request_id: &str) {
    let request = DirectMutationRequestV1::new(
        IdempotencyKeyV1::new(key).unwrap(),
        digest(),
        "workspace.repair",
        request_id,
        DirectOutcomeScopeV1::Workspace,
    )
    .unwrap();
    store
        .mutate(
            &request,
            deadline(),
            || UnixMillis::new(NOW + 5),
            |transaction| {
                transaction.now();
                transaction.create_graph(graph)?;
                Ok(mutate_outcome("workspace.repair", request_id))
            },
        )
        .unwrap();
}

fn with_current<T>(
    state: &GraphSessionStateV2,
    body: impl FnOnce(&DirectWorkspaceViewV1) -> T,
) -> T {
    let (_fixture, store) = provisioned_store();
    seed_graph(&store, state, "seed-graph", SEED_REQUEST);
    store
        .read(deadline(), |snapshot| {
            let view = snapshot.view(UnixMillis::new(OBSERVED_AT))?;
            Ok(body(&view))
        })
        .unwrap()
}

fn with_archive<T>(
    state: &GraphSessionStateV2,
    body: impl FnOnce(&DirectWorkspaceViewV1, &SessionArchiveSummaryV2) -> T,
) -> T {
    let (_fixture, store) = provisioned_store();
    seed_graph(&store, state, "seed-graph", SEED_REQUEST);
    let disposition = TerminalDispositionV2::not_required(
        state.trace().session_id().clone(),
        state.trace().revision(),
        "No handoff is required.",
        None,
        UnixMillis::new(NOW + 6),
    )
    .unwrap();
    let request = DirectMutationRequestV1::new(
        IdempotencyKeyV1::new("archive-graph").unwrap(),
        digest(),
        "session.archive",
        ARCHIVE_REQUEST,
        DirectOutcomeScopeV1::Workspace,
    )
    .unwrap();
    store
        .mutate(
            &request,
            deadline(),
            || UnixMillis::new(NOW + 6),
            |transaction| {
                transaction.now();
                transaction.record_disposition(&disposition)?;
                transaction.archive_graph(state.workspace_revision(), state.trace().revision())?;
                Ok(mutate_outcome("session.archive", ARCHIVE_REQUEST))
            },
        )
        .unwrap();
    store
        .read(deadline(), |snapshot| {
            let view = snapshot
                .archived_view(state.trace().session_id(), UnixMillis::new(OBSERVED_AT))?
                .unwrap();
            let summary = snapshot
                .archive_summary(state.trace().session_id())?
                .unwrap();
            Ok(body(&view, &summary))
        })
        .unwrap()
}

fn graph_view(state: GraphSessionStateV2) -> GraphWorkspaceViewV2 {
    let digest = digest();
    GraphWorkspaceViewV2::new(
        DurableWorktreeIdentityV1::new(
            digest.clone(),
            WorkspaceId::new(WORKSPACE).unwrap(),
            digest,
        ),
        Some(state),
        1,
        UnixMillis::new(OBSERVED_AT),
    )
}

fn list_item_has_materialized_bound(canonical: &str) -> bool {
    let document: Value = serde_json::from_str(canonical).unwrap();
    document["node_definitions"]
        .as_object()
        .unwrap()
        .values()
        .filter_map(|definition| definition.get("items")?.as_array())
        .flatten()
        .any(|item| item["type"] == "list" && item["max_total_length"] == json!(1_000_000))
}

fn assert_projection_identity(
    view: &DirectWorkspaceViewV1,
    stored_digest: &str,
    stored_bytes: &str,
) {
    let status = project_direct_status_v1(view, GraphStatusTierV2::Compact, None).unwrap();
    assert_eq!(status["procedure"]["digest"], stored_digest);
    let next = project_direct_next_v1(view);
    match next {
        Ok(next) => assert_eq!(next["procedure_digest"], stored_digest),
        Err(GraphViewErrorV2::TerminalSessionHasNoNext) => {}
        Err(error) => panic!("unexpected next error: {error:?}"),
    }
    let observation = project_direct_observation_v1(view).unwrap();
    assert_eq!(observation["status"]["procedure"]["digest"], stored_digest);
    assert_eq!(
        view.graph().unwrap().snapshot().canonical_json().as_str(),
        stored_bytes
    );
    assert_eq!(
        view.graph().unwrap().snapshot().digest().as_str(),
        stored_digest
    );
}

#[test]
fn optim001_direct_identity_preserves_four_boundaries() {
    let equal_running = running(LIST);
    let equal_validated = revalidated(&equal_running);
    assert_eq!(
        equal_validated.digest().as_str(),
        equal_running.snapshot().digest().as_str()
    );
    assert_eq!(
        equal_validated.canonical_json().as_str(),
        equal_running.snapshot().canonical_json().as_str()
    );
    assert!(list_item_has_materialized_bound(
        equal_running.snapshot().canonical_json().as_str()
    ));

    let differing_running = strip_materialized_list_bound(running(LIST));
    let differing_validated = revalidated(&differing_running);
    assert_ne!(
        differing_validated.digest().as_str(),
        differing_running.snapshot().digest().as_str()
    );
    assert_ne!(
        differing_validated.canonical_json().as_str(),
        differing_running.snapshot().canonical_json().as_str()
    );
    assert!(list_item_has_materialized_bound(
        differing_validated.canonical_json().as_str()
    ));
    assert!(!list_item_has_materialized_bound(
        differing_running.snapshot().canonical_json().as_str()
    ));

    let equal_digest = equal_running.snapshot().digest().as_str().to_owned();
    let equal_bytes = equal_running
        .snapshot()
        .canonical_json()
        .as_str()
        .to_owned();
    with_current(&equal_running, |view| {
        assert_projection_identity(view, &equal_digest, &equal_bytes);
    });
    with_archive(&cancelled(LIST), |view, summary| {
        let digest = view
            .graph()
            .unwrap()
            .snapshot()
            .digest()
            .as_str()
            .to_owned();
        let bytes = view
            .graph()
            .unwrap()
            .snapshot()
            .canonical_json()
            .as_str()
            .to_owned();
        let archive =
            project_direct_archive_show_v1(view, summary, GraphStatusTierV2::Standard, None)
                .unwrap();
        assert_eq!(archive["state"]["procedure"]["digest"], digest);
        assert_eq!(
            view.graph().unwrap().snapshot().canonical_json().as_str(),
            bytes
        );
    });

    let differing_digest = differing_running.snapshot().digest().as_str().to_owned();
    let differing_bytes = differing_running
        .snapshot()
        .canonical_json()
        .as_str()
        .to_owned();
    with_current(&differing_running, |view| {
        assert_projection_identity(view, &differing_digest, &differing_bytes);
    });
    let differing_cancelled = strip_materialized_list_bound(cancelled(LIST));
    with_archive(&differing_cancelled, |view, summary| {
        let digest = view
            .graph()
            .unwrap()
            .snapshot()
            .digest()
            .as_str()
            .to_owned();
        let bytes = view
            .graph()
            .unwrap()
            .snapshot()
            .canonical_json()
            .as_str()
            .to_owned();
        assert_ne!(digest, differing_validated.digest().as_str());
        let archive =
            project_direct_archive_show_v1(view, summary, GraphStatusTierV2::Standard, None)
                .unwrap();
        assert_eq!(archive["state"]["procedure"]["digest"], digest);
        assert_eq!(
            view.graph().unwrap().snapshot().canonical_json().as_str(),
            bytes
        );
    });

    let delegated_list = delegated(LIST);
    let equal_delegated = running(delegated_list.as_slice());
    let equal_package_view = graph_view(equal_delegated.clone());
    let equal_package = delegation_work_package_from_view_v1(&equal_package_view).unwrap();
    assert_eq!(
        equal_package.package()["procedure_digest"]
            .as_str()
            .unwrap(),
        equal_delegated.snapshot().digest().as_str()
    );
    project_delegation_package_result_v1(&equal_package_view, &equal_package).unwrap();
    with_current(&equal_delegated, |view| {
        let package = project_direct_delegation_package_v1(view).unwrap();
        assert_eq!(
            package["package"]["procedure_digest"],
            equal_delegated.snapshot().digest().as_str()
        );
    });

    let differing_delegated = strip_materialized_list_bound(running(delegated_list.as_slice()));
    let differing_package_validated = revalidated(&differing_delegated);
    assert_ne!(
        differing_package_validated.digest(),
        differing_delegated.snapshot().digest()
    );
    let differing_package_view = graph_view(differing_delegated.clone());
    let differing_package = delegation_work_package_from_view_v1(&differing_package_view).unwrap();
    assert_eq!(
        differing_package.package()["procedure_digest"]
            .as_str()
            .unwrap(),
        differing_package_validated.digest().as_str()
    );
    assert_eq!(
        project_delegation_package_result_v1(&differing_package_view, &differing_package)
            .unwrap_err(),
        GraphViewErrorV2::InconsistentState(
            "delegation package does not match the selected attempt",
        )
    );
    with_current(&differing_delegated, |view| {
        assert_eq!(
            project_direct_delegation_package_v1(view).unwrap_err(),
            GraphViewErrorV2::InconsistentState(
                "delegation package does not match the selected attempt",
            )
        );
    });

    required_local_artifacts_v2(&equal_running).unwrap();
    assert!(matches!(
        required_local_artifacts_v2(&differing_running).unwrap_err(),
        ExecutionErrorV1::InvalidPersistedExecution {
            reason: "Procedure v2 snapshot identity is inconsistent",
        }
    ));
}

#[test]
fn optim001_direct_prepared_identity_keeps_stored_bytes() {
    let equal = prepared(LIST);
    let equal_validated = revalidated(&equal);
    assert_eq!(
        equal_validated.digest().as_str(),
        equal.snapshot().digest().as_str()
    );
    assert_eq!(
        equal_validated.canonical_json().as_str(),
        equal.snapshot().canonical_json().as_str()
    );
    with_current(&equal, |view| {
        let stored = view
            .graph()
            .unwrap()
            .snapshot()
            .digest()
            .as_str()
            .to_owned();
        let bytes = view
            .graph()
            .unwrap()
            .snapshot()
            .canonical_json()
            .as_str()
            .to_owned();
        let next = project_direct_next_v1(view).unwrap();
        assert_eq!(next["schema"], "podway.prepared-next-result/v3");
        assert_eq!(next["procedure_digest"], stored);
        let observation = project_direct_observation_v1(view).unwrap();
        assert_eq!(observation["status"]["procedure"]["digest"], stored);
        assert_eq!(
            view.graph().unwrap().snapshot().canonical_json().as_str(),
            bytes
        );
    });

    let differing = strip_materialized_list_bound(prepared(LIST));
    let differing_validated = revalidated(&differing);
    assert_ne!(
        differing_validated.digest().as_str(),
        differing.snapshot().digest().as_str()
    );
    assert_ne!(
        differing_validated.canonical_json().as_str(),
        differing.snapshot().canonical_json().as_str()
    );
    with_current(&differing, |view| {
        let stored = view
            .graph()
            .unwrap()
            .snapshot()
            .digest()
            .as_str()
            .to_owned();
        let bytes = view
            .graph()
            .unwrap()
            .snapshot()
            .canonical_json()
            .as_str()
            .to_owned();
        assert_ne!(stored, differing_validated.digest().as_str());
        let next = project_direct_next_v1(view).unwrap();
        assert_eq!(next["procedure_digest"], stored);
        let observation = project_direct_observation_v1(view).unwrap();
        assert_eq!(observation["status"]["procedure"]["digest"], stored);
        assert_eq!(
            view.graph().unwrap().snapshot().canonical_json().as_str(),
            bytes
        );
    });
}
