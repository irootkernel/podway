use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::Path,
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, Instant},
};

use nix::{
    sys::signal::{Signal, kill},
    unistd::{Pid, geteuid},
};
use podway_cli::client::DaemonClientV1;
use podway_core::UnixMillis;
use podway_protocol::{
    ClientInfoV1, CommandNameV1, OperationV1, PreconditionsV1, RequestEnvelopeInputV1,
    RequestEnvelopeV1, RequestIdV1, RequestOptionsV1, ResponseEnvelopeV1, build_identity_v1,
};
use podway_service::{
    FixedServiceClockV1, InstallSpecV1, LaunchctlOutputV1, LaunchctlRunnerV1, LocalPlatformPathV1,
    MacosServiceCommandRunnerV1, ServiceErrorV1, ServiceManagerContractV1, ServiceManagerV1,
    ServiceOperationV1, ServiceOutcomeKindV1, ServiceRuntimePathsV1, StdServiceFilesystemV1,
};
use serde_json::{Map, Value};

#[derive(Clone, Default)]
struct StatefulLaunchctl {
    calls: Arc<Mutex<Vec<Vec<String>>>>,
    loaded: Arc<AtomicBool>,
}

impl LaunchctlRunnerV1 for StatefulLaunchctl {
    fn run(&self, arguments: &[String]) -> Result<LaunchctlOutputV1, ServiceErrorV1> {
        self.calls
            .lock()
            .expect("launchctl call lock")
            .push(arguments.to_vec());
        match arguments.first().map(String::as_str) {
            Some("print") if self.loaded.load(Ordering::SeqCst) => Ok(LaunchctlOutputV1 {
                exit_status: 0,
                stdout: format!(
                    "{} = {{\n\tpid = 4242\n}}\n",
                    arguments.get(1).expect("launchctl print target")
                ),
                stderr: String::new(),
            }),
            Some("print") => Ok(LaunchctlOutputV1 {
                exit_status: 113,
                stdout: String::new(),
                stderr: format!(
                    "Bad request.\nCould not find service \"dev.podway.podwayd\" in domain for user gui: {}\n",
                    geteuid().as_raw()
                ),
            }),
            Some("bootstrap") => {
                self.loaded.store(true, Ordering::SeqCst);
                Ok(LaunchctlOutputV1::success())
            }
            Some("bootout") => {
                self.loaded.store(false, Ordering::SeqCst);
                Ok(LaunchctlOutputV1::success())
            }
            _ => Ok(LaunchctlOutputV1::success()),
        }
    }
}

fn spawn_daemon(daemon: &Path, home: &Path, paths: &ServiceRuntimePathsV1) -> Child {
    let child = Command::new(daemon)
        .args(["--service", "--socket"])
        .arg(paths.socket_path().as_path())
        .env_clear()
        .env("PODWAY_TEST_ACCOUNT_ROOT", home)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("verified daemon must start");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !paths.socket_path().as_path().exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(
        paths.socket_path().as_path().exists(),
        "verified daemon must bind its configured socket"
    );
    child
}

fn live_daemon_status(paths: &ServiceRuntimePathsV1) -> Value {
    let request = RequestEnvelopeV1::new(RequestEnvelopeInputV1 {
        request_id: RequestIdV1::new("123e4567-e89b-42d3-a456-426614174000").expect("request UUID"),
        client: ClientInfoV1::new("podway-e2e", env!("CARGO_PKG_VERSION"), std::process::id())
            .expect("matching client identity"),
        operation: OperationV1::Control,
        command: CommandNameV1::new("daemon.status").expect("daemon status command"),
        workspace: None,
        idempotency_key: None,
        preconditions: PreconditionsV1::default(),
        options: RequestOptionsV1::new(false, 0).expect("daemon status options"),
        payload: Map::new(),
    })
    .expect("daemon status request");
    match DaemonClientV1::new(paths.clone())
        .daemon_status(&request)
        .expect("matching daemon status exchange")
    {
        ResponseEnvelopeV1::Output(output) => output.result().clone().into(),
        ResponseEnvelopeV1::Error(error) => {
            panic!("matching daemon rejected status: {:?}", error.code())
        }
    }
}

fn stop_daemon(child: Child) {
    kill(Pid::from_raw(child.id() as i32), Signal::SIGTERM).expect("stop verified daemon");
    let output = child.wait_with_output().expect("verified daemon output");
    assert!(
        output.status.success(),
        "verified daemon shutdown failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn mismatched_install_then_verified_install_restores_daemon_operation() {
    let root = std::path::PathBuf::from(format!("/tmp/pci-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    let home = root.join("home");
    let paths = ServiceRuntimePathsV1::for_account_home(&home, geteuid().as_raw())
        .expect("fixture service paths");
    let daemon = Path::new(env!("CARGO_BIN_EXE_podway")).with_file_name("podwayd");
    let launchctl = StatefulLaunchctl::default();
    let runner = MacosServiceCommandRunnerV1::new(
        StdServiceFilesystemV1,
        launchctl.clone(),
        FixedServiceClockV1::new(UnixMillis::new(1)),
        geteuid().as_raw(),
    )
    .expect("service runner");
    let manager = ServiceManagerV1::new(
        runner,
        FixedServiceClockV1::new(UnixMillis::new(1)),
        paths.clone(),
    );
    let spec = InstallSpecV1::new(
        LocalPlatformPathV1::new(&daemon).expect("daemon binary path"),
        podway_service::ServiceLabelV1::podwayd(),
        paths.clone(),
        "podway",
        "sha256:0000000000000000000000000000000000000000000000000000000000000000",
    );

    assert!(matches!(
        manager.install(spec),
        Err(ServiceErrorV1::OperationFailureV1 {
            operation: ServiceOperationV1::Install,
            source,
        }) if matches!(
            source.as_ref(),
            ServiceErrorV1::ContractMismatchV1 {
                actual_product: Some(product),
                actual_manifest_digest: Some(digest),
                ..
            } if product == "podway" && digest != "sha256:0000000000000000000000000000000000000000000000000000000000000000"
        )
    ));
    assert!(!paths.launch_agent_path().as_path().exists());
    assert!(!paths.metadata_index_path().as_path().exists());
    assert!(launchctl.calls.lock().expect("launchctl calls").is_empty());
    assert!(
        !root.exists(),
        "contract rejection must precede service directory creation"
    );

    fs::create_dir_all(&home).expect("account home fixture");
    fs::set_permissions(&home, fs::Permissions::from_mode(0o700))
        .expect("account home must be private");
    let expected = build_identity_v1();
    let matching = InstallSpecV1::new(
        LocalPlatformPathV1::new(&daemon).expect("daemon binary path"),
        podway_service::ServiceLabelV1::podwayd(),
        paths.clone(),
        expected.product(),
        expected.contract_manifest_digest(),
    );
    assert_eq!(
        manager
            .install(matching)
            .expect("verified matching install must succeed")
            .kind(),
        ServiceOutcomeKindV1::ChangedV1
    );
    assert!(paths.launch_agent_path().as_path().exists());
    assert!(paths.metadata_index_path().as_path().exists());
    assert!(
        launchctl
            .calls
            .lock()
            .expect("launchctl calls")
            .iter()
            .any(|arguments| arguments.first().is_some_and(|value| value == "bootstrap"))
    );

    let first_child = spawn_daemon(&daemon, &home, &paths);
    let first = live_daemon_status(&paths);
    assert_eq!(first["pid"], first_child.id());
    assert_eq!(
        first["contract_manifest_digest"],
        expected.contract_manifest_digest()
    );
    stop_daemon(first_child);

    let replacement = spawn_daemon(&daemon, &home, &paths);
    let restarted = live_daemon_status(&paths);
    assert_eq!(restarted["pid"], replacement.id());
    assert_ne!(restarted["process_id"], first["process_id"]);
    assert_eq!(
        restarted["contract_manifest_digest"],
        first["contract_manifest_digest"]
    );
    stop_daemon(replacement);
    fs::remove_dir_all(root).expect("remove contract identity fixture");
}
