//! Real Store commits followed by production response-reconstruction failures.

use std::{
    env, fs,
    os::unix::fs::PermissionsExt,
    path::Path,
    process::Command,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

use podway_core::RuntimeModeV1;
use podway_daemon::{
    production::compose_dispatcher_v1, runtime_workspace::WorkspaceRuntimeManagerV1,
};
use podway_protocol::{PreconditionsV1, ResponseEnvelopeV2, WorktreeSelectorWireV1};
use podway_service::ServiceRuntimePathsV1;
use podway_store::{
    IdempotencyKeyV1, PersistedTerminalReceiptV1, SqliteStoreOptionsV1, SqliteStoreV1,
    StoreErrorV1, WorkerIdV1, install_terminal_envelope_sealer_v1,
};
use serde_json::{Map, Value, json};

use super::{int_v2run003_runtime as runtime, support_phase4_workspace};

const CHILD: &str = "int_post_admission_reconstruction::reconstruction_child";
const CASE_ENV: &str = "PODWAY_RECONSTRUCTION_TEST_CASE";

fn malformed_sealer(_: &PersistedTerminalReceiptV1) -> Result<Value, StoreErrorV1> {
    // Store owns durable receipt validity; daemon owns the public envelope contract.
    // Keep the Store envelope schema valid and let production detect the missing fields.
    Ok(json!({"schema": "podway.output/v3"}))
}

fn manager(root: &Path) -> WorkspaceRuntimeManagerV1 {
    let home = root.join("home");
    let dev = root.join("dev");
    for directory in [&home, &dev] {
        fs::create_dir_all(directory).unwrap();
        fs::set_permissions(directory, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let paths =
        ServiceRuntimePathsV1::for_dev_home(&home, &dev, nix::unistd::geteuid().as_raw()).unwrap();
    WorkspaceRuntimeManagerV1::with_observability_and_scope(
        &paths,
        SqliteStoreOptionsV1::new(8).unwrap(),
        None,
        Some(fs::canonicalize(root).unwrap()),
    )
}

fn assert_response(response: &ResponseEnvelopeV2, injected: bool, request_id: &str) -> Value {
    // Exercise the public decoder too: a printed JSON object alone is not a valid response.
    let encoded = serde_json::to_vec(response).unwrap();
    podway_protocol::decode_response_payload_v2(&encoded).unwrap();
    let value: Value = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(value["request_id"], request_id);
    if injected {
        assert_eq!(value["code"], "INTERNAL_ERROR", "{value}");
        assert_eq!(value["exit_code"], 6);
        assert_eq!(value["retryable"], false);
        assert_eq!(
            value["details"]["schema"],
            "podway.internal-error-details/v1"
        );
        assert_eq!(
            value["details"]["kind"],
            "ADMITTED_RESPONSE_RECONSTRUCTION_FAILED"
        );
        assert_eq!(value["details"]["diagnostic_id"], request_id);
        assert_eq!(value["details"]["admission"]["admitted"], true);
        assert_eq!(
            value["details"]["job_id"],
            value["details"]["admission"]["job_id"]
        );
        assert_eq!(
            value["details"]["job_sequence"],
            value["details"]["admission"]["workspace_sequence"]
        );
    } else {
        assert_eq!(value["schema"], "podway.output/v3", "{value}");
    }
    value
}

#[test]
fn committed_response_failures_preserve_identity_and_replay() {
    // The sealer is process-global and installed once. Never replace it in the suite process.
    for case in ["start-control", "start-failure", "reset-control"] {
        let mut child = Command::new(env::current_exe().unwrap())
            .args(["--exact", CHILD, "--nocapture"])
            .env(CASE_ENV, case)
            .env("TMPDIR", "/private/tmp")
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                assert!(status.success(), "{case} child failed: {status}");
                break;
            }
            if Instant::now() >= deadline {
                child.kill().unwrap();
                child.wait().unwrap();
                panic!("{case} exceeded its bounded execution deadline");
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
}

#[test]
fn reconstruction_child() {
    let Ok(case) = env::var(CASE_ENV) else { return };
    assert!(matches!(
        case.as_str(),
        "start-control" | "start-failure" | "reset-control"
    ));
    let injected = case.ends_with("failure");
    if injected {
        install_terminal_envelope_sealer_v1(malformed_sealer);
    }
    let fixture = support_phase4_workspace::git_worktrees();
    runtime::make_runtime_private(fixture.main());
    let root = fixture.temporary_path();
    let worktree = fixture.main();
    let manager = Arc::new(manager(root));
    let dispatcher = compose_dispatcher_v1(
        Arc::clone(&manager),
        WorkerIdV1::new("reconstruction").unwrap(),
    );
    let selector = runtime::selector(worktree);
    let initialize = runtime::request(
        96_000,
        "workspace.init",
        &selector,
        Map::new(),
        "initialize",
        PreconditionsV1::default(),
    );
    let initialized = runtime::dispatch(&dispatcher, &initialize);
    let ResponseEnvelopeV2::OutputV2(initialized) = initialized else {
        panic!("{initialized:?}")
    };
    let old_uuid = initialized.workspace().unwrap().uuid().clone();
    let selector = WorktreeSelectorWireV1::new(
        &selector.path_bytes().unwrap(),
        selector.display(),
        Some(old_uuid.clone()),
    )
    .unwrap();
    let (command, payload) = if case.starts_with("reset") {
        (
            "workspace.reset_all",
            json!({"confirmed": true, "expected_workspace_uuid": old_uuid}),
        )
    } else {
        (
            "session.start",
            json!({"preset": "small-change-v2", "task_title": "Verify committed response recovery"}),
        )
    };
    let request = runtime::request(
        96_001,
        command,
        &selector,
        payload.as_object().unwrap().clone(),
        "reconstruct",
        PreconditionsV1::default(),
    );
    // Initialization can return while its worker is releasing the final claim.
    // Retry only an explicitly unadmitted, retryable maintenance response.
    let deadline = Instant::now() + Duration::from_secs(10);
    let terminal = loop {
        let response = runtime::dispatch(&dispatcher, &request);
        let value = serde_json::to_value(&response).unwrap();
        if value["code"] != "WORKSPACE_MAINTENANCE"
            || value["retryable"] != true
            || value["details"]["admission"]["admitted"] != false
            || Instant::now() >= deadline
        {
            break response;
        }
        thread::sleep(Duration::from_millis(10));
    };
    let response = assert_response(&terminal, injected, request.0.request_id().as_str());

    let database = worktree.join(".podway/runtime/state.sqlite3");
    let options = SqliteStoreOptionsV1::new(8)
        .unwrap()
        .with_runtime_mode(RuntimeModeV1::development());
    let binding = SqliteStoreV1::inspect_workspace_binding(&database, &options)
        .unwrap()
        .unwrap();
    if case.starts_with("reset") {
        assert_ne!(binding.identity().workspace_uuid(), &old_uuid);
    } else {
        assert_eq!(binding.identity().workspace_uuid(), &old_uuid);
    }
    let inspect = || {
        SqliteStoreV1::inspect_reconciliation_snapshot(
            &database,
            binding.identity(),
            &options,
            &IdempotencyKeyV1::new("reconstruct").unwrap(),
            runtime::observation().store_now(),
        )
        .unwrap()
    };
    let committed = inspect();
    let lookup = committed.lookup().unwrap();
    let receipt = lookup
        .terminal_receipt()
        .expect("terminal receipt must really be committed");
    let job = receipt.job();
    if injected {
        assert_eq!(response["details"]["job_id"], job.job_id().as_str());
        assert_eq!(response["details"]["job_sequence"], job.identity_sequence());
        assert_eq!(
            receipt.public_terminal_envelope().unwrap(),
            &json!({"schema": "podway.output/v3"})
        );
    } else {
        assert_eq!(response["job"]["id"], job.job_id().as_str());
    }
    let graph = SqliteStoreV1::inspect_graph_workspace_view_v2(
        &database,
        binding.identity(),
        &options,
        runtime::observation().store_now(),
    )
    .unwrap();
    assert_eq!(graph.graph_state().is_some(), case.starts_with("start"));

    let replay = runtime::request(
        96_002,
        command,
        &selector,
        payload.as_object().unwrap().clone(),
        "reconstruct",
        PreconditionsV1::default(),
    );
    let repeated = assert_response(
        &runtime::dispatch(&dispatcher, &replay),
        injected,
        replay.0.request_id().as_str(),
    );
    if injected {
        assert_eq!(
            response["details"]["admission"],
            repeated["details"]["admission"]
        );
    } else {
        assert_eq!(response["job"]["id"], repeated["job"]["id"]);
    }
    let after = inspect();
    assert_eq!(
        after.latest_workspace_sequence(),
        committed.latest_workspace_sequence()
    );
    assert_eq!(after.lookup().unwrap().terminal_receipt(), Some(receipt));
    let after_graph = SqliteStoreV1::inspect_graph_workspace_view_v2(
        &database,
        binding.identity(),
        &options,
        runtime::observation().store_now(),
    )
    .unwrap();
    assert_eq!(after_graph.graph_state(), graph.graph_state());
    drop(dispatcher);
    drop(manager);
    let cold = inspect();
    assert_eq!(cold.lookup().unwrap().terminal_receipt(), Some(receipt));
}
