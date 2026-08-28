//! V2RET-004 production workspace-removal lifecycle.

use std::fs;

use podway_core::WorkspaceId;
use podway_protocol::{PreconditionsV1, ResponseEnvelopeV2, WorktreeSelectorWireV1};
use serde_json::{Map, Value, json};

use crate::{int_v2run003_runtime, support_phase4_workspace};

fn selector_with_uuid(
    selector: &WorktreeSelectorWireV1,
    workspace_uuid: WorkspaceId,
) -> WorktreeSelectorWireV1 {
    let selector_path = selector.path_bytes().unwrap();
    WorktreeSelectorWireV1::new(&selector_path, selector.display(), Some(workspace_uuid)).unwrap()
}

#[test]
fn confirmed_removal_deletes_only_podway_and_converges_on_replay() {
    let fixture = support_phase4_workspace::git_worktrees();
    int_v2run003_runtime::make_runtime_private(fixture.main());
    let selector = int_v2run003_runtime::selector(fixture.main());
    let manager = std::sync::Arc::new(int_v2run003_runtime::manager(fixture.temporary_path()));
    let dispatcher =
        int_v2run003_runtime::dispatcher(std::sync::Arc::clone(&manager), "v2ret004-worker");
    let initialize = int_v2run003_runtime::request(
        400_001,
        "workspace.init",
        &selector,
        Map::new(),
        "v2ret004-initialize",
        PreconditionsV1::default(),
    );
    let initialized = int_v2run003_runtime::dispatch(&dispatcher, &initialize);
    let ResponseEnvelopeV2::OutputV2(initialized) = initialized else {
        panic!("workspace.init must succeed")
    };
    let workspace_uuid = initialized.workspace().unwrap().uuid().clone();
    let selected = selector_with_uuid(&selector, workspace_uuid.clone());

    let outside = fixture.main().join("preserved.txt");
    fs::write(&outside, "preserve the Git worktree\n").unwrap();
    let custom = fixture.main().join(".podway/custom/procedure.yaml");
    fs::create_dir_all(custom.parent().unwrap()).unwrap();
    fs::write(&custom, "schema: custom\n").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, fixture.main().join(".podway/outside-link")).unwrap();

    let remove = int_v2run003_runtime::request(
        400_002,
        "workspace.remove",
        &selected,
        json!({"force": true, "confirmed": true})
            .as_object()
            .unwrap()
            .clone(),
        "unused-for-control",
        PreconditionsV1::default(),
    );
    let removed = int_v2run003_runtime::v2_result(
        int_v2run003_runtime::dispatch(&dispatcher, &remove),
        "workspace.remove",
    );
    assert_eq!(removed["schema"], "podway.workspace-removal-result/v1");
    assert_eq!(removed["workspace_uuid"], workspace_uuid.as_str());
    assert_eq!(removed["registry_entry_removed"], true);
    assert_eq!(removed["podway_directory_removed"], true);
    assert_eq!(removed["already_absent"], false);
    assert!(!fixture.main().join(".podway").exists());
    assert_eq!(
        fs::read_to_string(&outside).unwrap(),
        "preserve the Git worktree\n"
    );
    assert!(fixture.main().join(".git").exists());

    let replayed = int_v2run003_runtime::v2_result(
        int_v2run003_runtime::dispatch(&dispatcher, &remove),
        "workspace.remove",
    );
    assert_eq!(replayed["workspace_uuid"], Value::Null);
    assert_eq!(replayed["registry_entry_removed"], false);
    assert_eq!(replayed["podway_directory_removed"], false);
    assert_eq!(replayed["already_absent"], true);

    let reinitialize = int_v2run003_runtime::request(
        400_003,
        "workspace.init",
        &selector,
        Map::new(),
        "v2ret004-reinitialize",
        PreconditionsV1::default(),
    );
    let response = int_v2run003_runtime::dispatch(&dispatcher, &reinitialize);
    let ResponseEnvelopeV2::OutputV2(reinitialized) = response else {
        panic!("a removed worktree must initialize as a new workspace: {response:?}")
    };
    assert_ne!(reinitialized.workspace().unwrap().uuid(), &workspace_uuid);
}

#[test]
fn uuid_mismatch_rejects_removal_without_mutation() {
    let fixture = support_phase4_workspace::git_worktrees();
    int_v2run003_runtime::make_runtime_private(fixture.main());
    let selector = int_v2run003_runtime::selector(fixture.main());
    let manager = std::sync::Arc::new(int_v2run003_runtime::manager(fixture.temporary_path()));
    let dispatcher = int_v2run003_runtime::dispatcher(manager, "v2ret004-mismatch-worker");
    let initialize = int_v2run003_runtime::request(
        400_011,
        "workspace.init",
        &selector,
        Map::new(),
        "v2ret004-mismatch-initialize",
        PreconditionsV1::default(),
    );
    assert!(matches!(
        int_v2run003_runtime::dispatch(&dispatcher, &initialize),
        ResponseEnvelopeV2::OutputV2(_)
    ));
    let mismatched = selector_with_uuid(
        &selector,
        WorkspaceId::new("00000000-0000-4000-8000-000000049999").unwrap(),
    );
    let remove = int_v2run003_runtime::request(
        400_012,
        "workspace.remove",
        &mismatched,
        json!({"force": true, "confirmed": true})
            .as_object()
            .unwrap()
            .clone(),
        "unused-for-control",
        PreconditionsV1::default(),
    );
    let ResponseEnvelopeV2::Error(error) = int_v2run003_runtime::dispatch(&dispatcher, &remove)
    else {
        panic!("a mismatched UUID must reject removal")
    };
    assert_eq!(error.code().as_str(), "WORKSPACE_UUID_MISMATCH");
    assert!(
        fixture
            .main()
            .join(".podway/runtime/state.sqlite3")
            .is_file()
    );
}

#[test]
fn markerless_residual_converges_only_when_descriptor_verified_empty() {
    let fixture = support_phase4_workspace::git_worktrees();
    let selector = selector_with_uuid(
        &int_v2run003_runtime::selector(fixture.main()),
        WorkspaceId::new("00000000-0000-4000-8000-000000040020").unwrap(),
    );
    let manager = std::sync::Arc::new(int_v2run003_runtime::manager(fixture.temporary_path()));
    let dispatcher = int_v2run003_runtime::dispatcher(manager, "v2ret004-residual-worker");
    let podway = fixture.main().join(".podway");
    fs::create_dir_all(podway.join("empty/leaf")).unwrap();
    let remove = int_v2run003_runtime::request(
        400_021,
        "workspace.remove",
        &selector,
        json!({"force": true, "confirmed": true})
            .as_object()
            .unwrap()
            .clone(),
        "unused-for-control",
        PreconditionsV1::default(),
    );
    let empty = int_v2run003_runtime::v2_result(
        int_v2run003_runtime::dispatch(&dispatcher, &remove),
        "workspace.remove",
    );
    assert_eq!(empty["already_absent"], true);
    assert_eq!(empty["podway_directory_removed"], true);
    assert!(!podway.exists());

    fs::create_dir(&podway).unwrap();
    fs::write(podway.join("ambiguous"), "retain without marker").unwrap();
    let ResponseEnvelopeV2::Error(error) = int_v2run003_runtime::dispatch(&dispatcher, &remove)
    else {
        panic!("a markerless nonempty residual must fail closed")
    };
    assert_eq!(error.code().as_str(), "WORKSPACE_PATH_UNSAFE");
    assert_eq!(
        fs::read_to_string(podway.join("ambiguous")).unwrap(),
        "retain without marker"
    );
}
