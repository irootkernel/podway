//! V2RET-005 crash recovery and observability conformance for workspace removal.

use std::{
    env, io,
    os::unix::process::ExitStatusExt,
    path::{Path, PathBuf},
    process::{Command, Output},
    sync::{Arc, Mutex},
};

use podway_core::WorkspaceId;
use podway_daemon::{
    ClockErrorV1, ClockV1, LogSinkV1, ObservabilityV1,
    runtime_workspace::WorkspaceRemovalCrashBoundaryV1,
};
use podway_protocol::{JobStateV1, PreconditionsV1, ResponseEnvelopeV2, WorktreeSelectorWireV1};
use serde_json::{Map, Value, json};

use crate::{int_v2run003_runtime, support_phase4_workspace};

const CRASH_CHILD_TEST_NAME: &str =
    "int_v2ret005_retirement_conformance::workspace_removal_crash_child";
const CRASH_WORKTREE_ENV: &str = "PODWAY_V2RET005_CRASH_WORKTREE";
const CRASH_SERVICE_ROOT_ENV: &str = "PODWAY_V2RET005_CRASH_SERVICE_ROOT";
const CRASH_WORKSPACE_UUID_ENV: &str = "PODWAY_V2RET005_CRASH_WORKSPACE_UUID";
const CRASH_BOUNDARY_ENV: &str = "PODWAY_V2RET005_CRASH_BOUNDARY";

fn selector_with_uuid(path: &Path, workspace_uuid: WorkspaceId) -> WorktreeSelectorWireV1 {
    let selector = int_v2run003_runtime::selector(path);
    WorktreeSelectorWireV1::new(
        &selector.path_bytes().unwrap(),
        selector.display(),
        Some(workspace_uuid),
    )
    .unwrap()
}

fn initialize_workspace(worktree: &Path, service_root: &Path, request_number: u64) -> WorkspaceId {
    int_v2run003_runtime::make_runtime_private(worktree);
    let selector = int_v2run003_runtime::selector(worktree);
    let manager = Arc::new(int_v2run003_runtime::manager(service_root));
    let dispatcher = int_v2run003_runtime::dispatcher(Arc::clone(&manager), "v2ret005-initialize");
    let request = int_v2run003_runtime::request(
        request_number,
        "workspace.init",
        &selector,
        Map::new(),
        "v2ret005-initialize",
        PreconditionsV1::default(),
    );
    let ResponseEnvelopeV2::OutputV2(output) =
        int_v2run003_runtime::dispatch(&dispatcher, &request)
    else {
        panic!("workspace initialization must succeed")
    };
    assert_eq!(
        output
            .job()
            .expect("workspace.init must return its job")
            .state(),
        JobStateV1::Succeeded,
        "workspace.init must reach a terminal success before crash testing"
    );
    let workspace_uuid = output.workspace().unwrap().uuid().clone();
    drop(dispatcher);
    drop(manager);

    let reopened = int_v2run003_runtime::manager(service_root);
    assert!(
        reopened
            .registry()
            .lookup(&workspace_uuid)
            .expect("cold registry read must succeed")
            .is_some(),
        "workspace.init must be durable before the crash child starts"
    );
    workspace_uuid
}

fn removal_request(
    path: &Path,
    workspace_uuid: WorkspaceId,
    request_number: u64,
) -> (
    podway_protocol::RequestEnvelopeV1,
    podway_daemon::server::DaemonRequestV1,
) {
    int_v2run003_runtime::request(
        request_number,
        "workspace.remove",
        &selector_with_uuid(path, workspace_uuid),
        json!({"force": true, "confirmed": true})
            .as_object()
            .unwrap()
            .clone(),
        "unused-for-control",
        PreconditionsV1::default(),
    )
}

fn crash_boundary_name(boundary: WorkspaceRemovalCrashBoundaryV1) -> &'static str {
    match boundary {
        WorkspaceRemovalCrashBoundaryV1::MarkerCreated => "marker-created",
        WorkspaceRemovalCrashBoundaryV1::RegistryEntryRemoved => "registry-entry-removed",
        WorkspaceRemovalCrashBoundaryV1::PodwayDirectoryRemoved => "podway-directory-removed",
    }
}

fn parse_crash_boundary(value: &str) -> WorkspaceRemovalCrashBoundaryV1 {
    match value {
        "marker-created" => WorkspaceRemovalCrashBoundaryV1::MarkerCreated,
        "registry-entry-removed" => WorkspaceRemovalCrashBoundaryV1::RegistryEntryRemoved,
        "podway-directory-removed" => WorkspaceRemovalCrashBoundaryV1::PodwayDirectoryRemoved,
        other => panic!("unknown workspace-removal crash boundary {other}"),
    }
}

fn run_crash_child(
    worktree: &Path,
    service_root: &Path,
    workspace_uuid: &WorkspaceId,
    boundary: WorkspaceRemovalCrashBoundaryV1,
) -> Output {
    Command::new(env::current_exe().unwrap())
        .arg("--exact")
        .arg(CRASH_CHILD_TEST_NAME)
        .arg("--nocapture")
        .env(CRASH_WORKTREE_ENV, worktree)
        .env(CRASH_SERVICE_ROOT_ENV, service_root)
        .env(CRASH_WORKSPACE_UUID_ENV, workspace_uuid.as_str())
        .env(CRASH_BOUNDARY_ENV, crash_boundary_name(boundary))
        .output()
        .expect("workspace-removal crash child must start")
}

#[test]
fn workspace_removal_crash_child() {
    let Ok(worktree) = env::var_os(CRASH_WORKTREE_ENV).ok_or(()) else {
        return;
    };
    let service_root = PathBuf::from(env::var_os(CRASH_SERVICE_ROOT_ENV).unwrap());
    let workspace_uuid = WorkspaceId::new(env::var(CRASH_WORKSPACE_UUID_ENV).unwrap()).unwrap();
    let boundary = parse_crash_boundary(&env::var(CRASH_BOUNDARY_ENV).unwrap());
    let manager = Arc::new(
        int_v2run003_runtime::manager_with_workspace_removal_crash_boundary(
            &service_root,
            boundary,
        ),
    );
    let dispatcher = int_v2run003_runtime::dispatcher(manager, "v2ret005-crash-child");
    let request = removal_request(Path::new(&worktree), workspace_uuid, 500_002);
    let response = int_v2run003_runtime::dispatch(&dispatcher, &request);
    panic!("configured workspace-removal crash boundary returned without aborting: {response:?}");
}

#[test]
fn every_removal_crash_boundary_converges_on_cold_replay() {
    for (sequence, boundary) in [
        WorkspaceRemovalCrashBoundaryV1::MarkerCreated,
        WorkspaceRemovalCrashBoundaryV1::RegistryEntryRemoved,
        WorkspaceRemovalCrashBoundaryV1::PodwayDirectoryRemoved,
    ]
    .into_iter()
    .enumerate()
    {
        let fixture = support_phase4_workspace::git_worktrees();
        let workspace_uuid = initialize_workspace(
            fixture.main(),
            fixture.temporary_path(),
            500_010 + sequence as u64 * 10,
        );
        let output = run_crash_child(
            fixture.main(),
            fixture.temporary_path(),
            &workspace_uuid,
            boundary,
        );
        assert_eq!(
            output.status.signal(),
            Some(6),
            "{} child must abort: stdout={} stderr={}",
            crash_boundary_name(boundary),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        );

        let manager = Arc::new(int_v2run003_runtime::manager(fixture.temporary_path()));
        let dispatcher =
            int_v2run003_runtime::dispatcher(Arc::clone(&manager), "v2ret005-recovery");
        let replay = removal_request(
            fixture.main(),
            workspace_uuid.clone(),
            500_011 + sequence as u64 * 10,
        );
        let recovered = int_v2run003_runtime::v2_result(
            int_v2run003_runtime::dispatch(&dispatcher, &replay),
            "workspace.remove",
        );
        assert_eq!(recovered["schema"], "podway.workspace-removal-result/v1");
        assert!(!fixture.main().join(".podway").exists());
        assert!(fixture.main().join(".git").exists());
        assert!(
            manager
                .registry()
                .lookup(&workspace_uuid)
                .unwrap()
                .is_none()
        );
    }
}

struct FixedClock;

impl ClockV1 for FixedClock {
    fn unix_seconds(&self) -> Result<u64, ClockErrorV1> {
        Ok(42)
    }
}

#[derive(Default)]
struct CapturingSink {
    events: Mutex<Vec<String>>,
}

impl LogSinkV1 for CapturingSink {
    fn write_event(&self, event: &str) -> io::Result<()> {
        self.events.lock().unwrap().push(event.to_owned());
        Ok(())
    }

    fn flush(&self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn removal_emits_bounded_rejected_and_succeeded_events() {
    let fixture = support_phase4_workspace::git_worktrees();
    let workspace_uuid = initialize_workspace(fixture.main(), fixture.temporary_path(), 500_100);
    let sink = Arc::new(CapturingSink::default());
    let observability = ObservabilityV1::start(sink.clone(), Arc::new(FixedClock));
    let manager = Arc::new(int_v2run003_runtime::manager(fixture.temporary_path()));
    let dispatcher = int_v2run003_runtime::dispatcher_with_observability(
        manager,
        "v2ret005-observability",
        observability.emitter(),
    );

    let mismatch = removal_request(
        fixture.main(),
        WorkspaceId::new("00000000-0000-4000-8000-000000059999").unwrap(),
        500_101,
    );
    assert!(matches!(
        int_v2run003_runtime::dispatch(&dispatcher, &mismatch),
        ResponseEnvelopeV2::Error(_)
    ));
    let removal = removal_request(fixture.main(), workspace_uuid.clone(), 500_102);
    assert!(matches!(
        int_v2run003_runtime::dispatch(&dispatcher, &removal),
        ResponseEnvelopeV2::OutputV2(_)
    ));
    drop(dispatcher);
    observability.shutdown();

    let events = sink
        .events
        .lock()
        .unwrap()
        .iter()
        .map(|event| serde_json::from_str::<Value>(event).unwrap())
        .filter(|event| event["operation"] == "workspace_remove")
        .collect::<Vec<_>>();
    assert_eq!(events.len(), 2);
    assert_eq!(events[0]["outcome"], "rejected");
    assert_eq!(events[1]["outcome"], "succeeded");
    for event in events {
        assert!(event.get("request_id").is_some());
        assert!(event.get("workspace_uuid").is_some());
        assert!(event.get("root").is_none());
        assert!(event.get("removed_content").is_none());
    }
}
