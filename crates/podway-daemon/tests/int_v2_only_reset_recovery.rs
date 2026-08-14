//! Isolated real-runtime recovery from an unsupported Procedure v1 predecessor.

use super::{int_v2run003_runtime as runtime, support_phase4_workspace};

use std::{fs, path::Path, sync::Arc};

use podway_core::WorkspaceId;
use podway_protocol::{PreconditionsV1, ResponseEnvelopeV2, WorktreeSelectorWireV1};
use serde_json::{Map, json};

fn selector_with_workspace(path: &Path, workspace: WorkspaceId) -> WorktreeSelectorWireV1 {
    let canonical = fs::canonicalize(path).unwrap();
    WorktreeSelectorWireV1::new(
        canonical.to_string_lossy().as_bytes(),
        canonical.display().to_string(),
        Some(workspace),
    )
    .unwrap()
}

#[test]
fn v2cut_legacy_schema_v3_rejects_then_confirmed_reset_all_replaces_and_cold_reopens() {
    let fixture = support_phase4_workspace::git_worktrees();
    runtime::make_runtime_private(fixture.main());
    let manager = Arc::new(runtime::manager(fixture.temporary_path()));
    let initialized = manager
        .bootstrap(
            support_phase4_workspace::selector(fixture.main()),
            runtime::observation(),
        )
        .unwrap();
    let old_workspace = initialized
        .context_snapshot()
        .binding()
        .identity()
        .workspace_uuid()
        .clone();
    drop(initialized);
    drop(manager);

    let database_path = fixture.main().join(".podway/runtime/state.sqlite3");
    podway_store::test_support::downgrade_to_schema_v3_with_legacy_snapshot(
        &database_path,
        "podway.procedure/v1",
    )
    .unwrap();

    let selector = selector_with_workspace(fixture.main(), old_workspace.clone());
    let manager = Arc::new(runtime::manager(fixture.temporary_path()));
    let dispatcher = runtime::dispatcher(Arc::clone(&manager), "v2cut-reset-recovery");
    let rejected = runtime::request(
        95_002,
        "workspace.init",
        &selector,
        Map::new(),
        "v2cut-reset-rejected-open",
        PreconditionsV1::default(),
    );
    let rejected = runtime::dispatch(&dispatcher, &rejected);
    let ResponseEnvelopeV2::Error(error) = &rejected else {
        panic!("legacy Procedure v1 state must fail closed before reset: {rejected:?}");
    };
    assert_eq!(error.code().as_str(), "LEGACY_PROCEDURE_STATE_UNSUPPORTED");

    let reset = runtime::request(
        95_003,
        "workspace.reset_all",
        &selector,
        json!({
            "confirmed": true,
            "expected_workspace_uuid": old_workspace,
        })
        .as_object()
        .unwrap()
        .clone(),
        "v2cut-reset-confirmed",
        PreconditionsV1::default(),
    );
    let reset = runtime::dispatch(&dispatcher, &reset);
    let ResponseEnvelopeV2::OutputV2(reset_output) = &reset else {
        panic!("confirmed reset-all must replace the unsupported legacy store: {reset:?}");
    };
    let new_workspace = reset_output.workspace().unwrap().uuid().clone();
    assert_ne!(new_workspace, old_workspace);
    assert_eq!(reset_output.command().as_str(), "workspace.reset_all");
    drop(dispatcher);
    drop(manager);

    let selector = selector_with_workspace(fixture.main(), new_workspace.clone());
    let manager = Arc::new(runtime::manager(fixture.temporary_path()));
    let reopened = manager
        .resolve_existing(
            support_phase4_workspace::selector(fixture.main()),
            Some(&new_workspace),
            runtime::observation(),
        )
        .unwrap();
    assert_eq!(
        reopened
            .context_snapshot()
            .binding()
            .identity()
            .workspace_uuid(),
        &new_workspace
    );
    let dispatcher = runtime::dispatcher(Arc::clone(&manager), "v2cut-reset-cold-reopen");
    let show = runtime::request(
        95_004,
        "workspace.show",
        &selector,
        Map::new(),
        "",
        PreconditionsV1::default(),
    );
    let ResponseEnvelopeV2::OutputV2(shown) = runtime::dispatch(&dispatcher, &show) else {
        panic!("replacement workspace must cold-reopen through the production dispatcher");
    };
    assert_eq!(shown.workspace().unwrap().uuid(), &new_workspace);
    drop(dispatcher);
    drop(manager);

    assert!(
        podway_store::SqliteStoreV1::inspect_workspace_binding(
            database_path,
            &podway_store::SqliteStoreOptionsV1::new(8).unwrap(),
        )
        .unwrap()
        .is_some()
    );
}
