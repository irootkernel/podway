//! V2RET-003 production pre-mutation stale-registry cleanup.

use std::{fs, sync::Arc};

use podway_core::{AttemptId, Revision, SessionId};
use podway_daemon::{
    dispatch::{
        DispatchFailureKindV1, MutationAdmissionWorkerV1, MutationResponseContextV1,
        MutationWaitV1, WorkspaceRuntimeV1,
    },
    production::{
        NativeProductionClockV1, ProductionMutationWorkerV1, ProductionWorkspaceRuntimeV1,
    },
    server::DaemonRequestV1,
};
use podway_protocol::{PreconditionsV1, ResponseEnvelopeV2, WorktreeSelectorWireV1};
use podway_store::WorkerIdV1;
use serde_json::json;

use crate::{int_v2run003_runtime, support_phase4_workspace};

fn selector_with_uuid(
    selector: &WorktreeSelectorWireV1,
    workspace_uuid: podway_core::WorkspaceId,
) -> WorktreeSelectorWireV1 {
    let selector_path = selector.path_bytes().unwrap();
    WorktreeSelectorWireV1::new(&selector_path, selector.display(), Some(workspace_uuid)).unwrap()
}

#[test]
fn dispatcher_classifies_a_deleted_registered_worktree_before_session_begin_admission() {
    let fixture = support_phase4_workspace::git_worktrees();
    int_v2run003_runtime::make_runtime_private(fixture.main());
    let selector = int_v2run003_runtime::selector(fixture.main());
    let manager = Arc::new(int_v2run003_runtime::manager(fixture.temporary_path()));
    let dispatcher = int_v2run003_runtime::dispatcher(Arc::clone(&manager), "v2ret003-dispatch");
    let initialize = int_v2run003_runtime::request(
        300_000,
        "workspace.init",
        &selector,
        serde_json::Map::new(),
        "v2ret003-dispatch-initialize",
        PreconditionsV1::default(),
    );
    let ResponseEnvelopeV2::OutputV2(initialized) =
        int_v2run003_runtime::dispatch(&dispatcher, &initialize)
    else {
        panic!("workspace.init must succeed")
    };
    let workspace_uuid = initialized.workspace().unwrap().uuid().clone();
    let selected = selector_with_uuid(&selector, workspace_uuid.clone());
    let begin = int_v2run003_runtime::request(
        300_010,
        "session.begin",
        &selected,
        serde_json::Map::new(),
        "v2ret003-dispatch-begin",
        PreconditionsV1::new(
            Some(SessionId::new("00000000-0000-4000-8000-000000030010").unwrap()),
            Some(Revision::ZERO),
            None,
            None,
            None,
            None,
        )
        .unwrap(),
    );

    fs::remove_dir_all(fixture.main()).expect("fixture worktree must be deleted");
    let ResponseEnvelopeV2::Error(failure) = int_v2run003_runtime::dispatch(&dispatcher, &begin)
    else {
        panic!("deleted worktree must fail before session.begin admission")
    };
    assert_eq!(failure.code().as_str(), "WORKTREE_GONE");
    assert!(
        manager
            .registry()
            .load()
            .expect("registry must remain readable")
            .lookup(&workspace_uuid)
            .is_none(),
        "the exact deleted generation must be pruned"
    );
}

#[test]
fn pre_mutation_revalidation_retires_and_prunes_a_deleted_worktree() {
    let fixture = support_phase4_workspace::git_worktrees();
    int_v2run003_runtime::make_runtime_private(fixture.main());
    let selector = int_v2run003_runtime::selector(fixture.main());
    let manager = Arc::new(int_v2run003_runtime::manager(fixture.temporary_path()));
    let dispatcher = int_v2run003_runtime::dispatcher(Arc::clone(&manager), "v2ret003-init");
    let initialize = int_v2run003_runtime::request(
        300_001,
        "workspace.init",
        &selector,
        serde_json::Map::new(),
        "v2ret003-initialize",
        PreconditionsV1::default(),
    );
    assert!(matches!(
        int_v2run003_runtime::dispatch(&dispatcher, &initialize),
        ResponseEnvelopeV2::OutputV2(_)
    ));

    let clock = Arc::new(NativeProductionClockV1::default());
    let runtime = ProductionWorkspaceRuntimeV1::new(Arc::clone(&manager), Arc::clone(&clock));
    let workspace = runtime
        .resolve_existing(&selector)
        .expect("the scheduler must be selected before deletion");
    let worker = ProductionMutationWorkerV1::new(
        WorkerIdV1::new("v2ret003-worker").unwrap(),
        clock,
        Arc::clone(&manager),
    );
    let request = int_v2run003_runtime::request(
        300_002,
        "session.cancel",
        &selector,
        json!({"reason":"Confirm no mutation is admitted after worktree deletion."})
            .as_object()
            .unwrap()
            .clone(),
        "v2ret003-cancel",
        PreconditionsV1::new(
            Some(SessionId::new("00000000-0000-4000-8000-000000030002").unwrap()),
            Some(Revision::ZERO),
            Some(AttemptId::new("00000000-0000-4000-8000-000000030003").unwrap()),
            None,
            None,
            None,
        )
        .unwrap(),
    );
    let DaemonRequestV1::Legacy(slice) = &request.1 else {
        panic!("session.cancel must retain the legacy slice adapter")
    };
    let response_context =
        MutationResponseContextV1::new(&request.0, runtime.workspace_output(&workspace));
    let workspace_uuid = response_context.workspace().uuid().clone();

    fs::remove_dir_all(fixture.main()).expect("fixture worktree must be deleted");
    let observed = manager
        .revalidate_scheduler(workspace.scheduler())
        .expect("deleted-root revalidation must classify without an infrastructure failure");
    assert!(matches!(
        observed,
        podway_daemon::runtime_workspace::WorkspaceSchedulerRevalidationV1::RetireRequired { .. }
    ));
    let failure = worker
        .admit_and_wait(
            &workspace,
            slice,
            request.0.idempotency_key().unwrap(),
            &response_context,
            MutationWaitV1::Detached,
        )
        .expect_err("deleted worktree must fail before queue admission");
    assert_eq!(failure.kind(), DispatchFailureKindV1::WorktreeGone);
    assert!(
        manager
            .registry()
            .load()
            .expect("registry must remain readable")
            .lookup(&workspace_uuid)
            .is_none(),
        "the exact deleted generation must be pruned"
    );
}

#[test]
fn pre_mutation_revalidation_rejects_a_replaced_runtime_database() {
    let fixture = support_phase4_workspace::git_worktrees();
    int_v2run003_runtime::make_runtime_private(fixture.main());
    let selector = int_v2run003_runtime::selector(fixture.main());
    let manager = Arc::new(int_v2run003_runtime::manager(fixture.temporary_path()));
    let dispatcher = int_v2run003_runtime::dispatcher(Arc::clone(&manager), "v2ret003-init");
    let initialize = int_v2run003_runtime::request(
        300_101,
        "workspace.init",
        &selector,
        serde_json::Map::new(),
        "v2ret003-replaced-database-initialize",
        PreconditionsV1::default(),
    );
    assert!(matches!(
        int_v2run003_runtime::dispatch(&dispatcher, &initialize),
        ResponseEnvelopeV2::OutputV2(_)
    ));

    let clock = Arc::new(NativeProductionClockV1::default());
    let runtime = ProductionWorkspaceRuntimeV1::new(Arc::clone(&manager), Arc::clone(&clock));
    let workspace = runtime
        .resolve_existing(&selector)
        .expect("the scheduler must be selected before database replacement");
    let response_context =
        MutationResponseContextV1::new(&initialize.0, runtime.workspace_output(&workspace));
    let database = workspace
        .scheduler()
        .context_snapshot()
        .database_path()
        .to_path_buf();
    let replacement = database.with_extension("replacement");
    fs::copy(&database, &replacement).expect("replacement database fixture must copy");
    fs::rename(&replacement, &database).expect("replacement database must publish atomically");
    assert!(matches!(
        manager
            .revalidate_scheduler(workspace.scheduler())
            .expect("database replacement must produce a typed retirement signal"),
        podway_daemon::runtime_workspace::WorkspaceSchedulerRevalidationV1::RetireRequired { .. }
    ));

    let worker = ProductionMutationWorkerV1::new(
        WorkerIdV1::new("v2ret003-replaced-database-worker").unwrap(),
        clock,
        Arc::clone(&manager),
    );
    let request = int_v2run003_runtime::request(
        300_102,
        "session.cancel",
        &selector,
        json!({"reason":"Reject mutation after runtime database replacement."})
            .as_object()
            .unwrap()
            .clone(),
        "v2ret003-replaced-database-cancel",
        PreconditionsV1::new(
            Some(SessionId::new("00000000-0000-4000-8000-000000030102").unwrap()),
            Some(Revision::ZERO),
            Some(AttemptId::new("00000000-0000-4000-8000-000000030103").unwrap()),
            None,
            None,
            None,
        )
        .unwrap(),
    );
    let DaemonRequestV1::Legacy(slice) = &request.1 else {
        panic!("session.cancel must retain the legacy slice adapter")
    };
    let failure = worker
        .admit_and_wait(
            &workspace,
            slice,
            request.0.idempotency_key().unwrap(),
            &response_context,
            MutationWaitV1::Detached,
        )
        .expect_err("stale database identity must fail before queue admission");
    assert_eq!(
        failure.kind(),
        DispatchFailureKindV1::WorkspaceIdentityConflict
    );
}
