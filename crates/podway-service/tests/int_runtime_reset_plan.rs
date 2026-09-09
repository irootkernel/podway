use std::{
    fs::{self, File},
    os::unix::fs::{MetadataExt, PermissionsExt, symlink},
    path::{Path, PathBuf},
};

use nix::{
    fcntl::{Flock, FlockArg},
    unistd::geteuid,
};
use podway_core::{RuntimeModeV1, UnixMillis, canonicalize_json_v1};
use podway_service::*;
use serde_json::Value;
use sha2::{Digest, Sha256};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path =
            Path::new("/private/tmp").join(format!("pw-rp-{}", uuid::Uuid::new_v4().simple()));
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        Self(path.canonicalize().unwrap())
    }
    fn home(&self) -> PodwayHomeV1 {
        PodwayHomeV1::from_account_home(&self.0, geteuid().as_raw()).unwrap()
    }
    fn directory(&self, relative: &str) -> PathBuf {
        let mut path = self.0.clone();
        for component in Path::new(relative).components() {
            path.push(component);
            if !path.exists() {
                fs::create_dir(&path).unwrap();
            }
            fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        }
        path
    }
    fn file(&self, relative: &str, bytes: &[u8]) -> PathBuf {
        let relative = Path::new(relative);
        self.directory(relative.parent().unwrap().to_str().unwrap());
        let path = self.0.join(relative);
        fs::write(&path, bytes).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        path
    }
    fn namespace(&self, mode: &str) -> PathBuf {
        let root = if mode == "prod" {
            ".podway".to_owned()
        } else {
            format!(".podway/modes/{mode}")
        };
        self.file(&format!("{root}/run/podwayd.lock"), b"");
        self.file(
            &format!("{root}/state/workspaces.json"),
            b"corrupt registry, never parse or follow",
        );
        self.0.join(root)
    }
    fn planner(&self) -> RuntimeResetPlannerV1<Launchctl, Inspector> {
        RuntimeResetPlannerV1::new(
            self.home(),
            Launchctl(false),
            Inspector(RuntimeResetPeerV1::Offline),
        )
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
struct Launchctl(bool);
impl LaunchctlRunnerV1 for Launchctl {
    fn run(&self, arguments: &[String]) -> Result<LaunchctlOutputV1, ServiceErrorV1> {
        assert_eq!(
            arguments,
            &[
                "print",
                &format!("gui/{}/dev.podway.podwayd", geteuid().as_raw())
            ]
        );
        Ok(if self.0 {
            LaunchctlOutputV1 {
                exit_status: 0,
                stdout: format!("{} = {{\n}}\n", arguments[1]),
                stderr: String::new(),
            }
        } else {
            LaunchctlOutputV1 {
                exit_status: 113,
                stdout: String::new(),
                stderr: format!(
                    "Bad request.\nCould not find service \"dev.podway.podwayd\" in domain for user gui: {}",
                    geteuid().as_raw()
                ),
            }
        })
    }
}
struct Inspector(RuntimeResetPeerV1);
impl RuntimeResetInspectorV1 for Inspector {
    fn inspect(
        &mut self,
        _: &ServiceRuntimePathsV1,
    ) -> Result<RuntimeResetPeerV1, RuntimeResetErrorV1> {
        Ok(self.0.clone())
    }
}
fn mode(value: &str) -> RuntimeResetSelectionV1 {
    RuntimeResetSelectionV1::Mode {
        mode: RuntimeModeV1::new(value).unwrap(),
    }
}
fn now() -> UnixMillis {
    UnixMillis::new(1000)
}

fn encode_base64url_unpadded(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut encoded = String::with_capacity((bytes.len() * 4).div_ceil(3));
    for chunk in bytes.chunks(3) {
        encoded.push(ALPHABET[(chunk[0] >> 2) as usize] as char);
        encoded.push(
            ALPHABET[(((chunk[0] & 3) << 4) | (chunk.get(1).copied().unwrap_or(0) >> 4)) as usize]
                as char,
        );
        if chunk.len() > 1 {
            encoded.push(
                ALPHABET
                    [(((chunk[1] & 15) << 2) | (chunk.get(2).copied().unwrap_or(0) >> 6)) as usize]
                    as char,
            );
        }
        if chunk.len() > 2 {
            encoded.push(ALPHABET[(chunk[2] & 63) as usize] as char);
        }
    }
    encoded
}

#[test]
fn runtime_reset_lock_anchors_are_private_retained_and_exclusive() {
    let fixture = Fixture::new();
    let home = fixture.home();
    let guard = RuntimeResetLockV1::acquire_reset(&home).unwrap();
    let path = home.as_path().join("maintenance/runtime-reset.lock");
    let before = fs::metadata(&path).unwrap();
    assert_eq!(before.mode() & 0o7777, 0o600);
    assert_eq!(before.nlink(), 1);
    assert!(
        matches!(RuntimeResetLockV1::acquire_reset(&home), Err(error) if error.code() == "RUNTIME_RESET_IN_PROGRESS")
    );
    drop(guard);
    let _again = RuntimeResetLockV1::acquire_reset(&home).unwrap();
    assert_eq!(fs::metadata(path).unwrap().ino(), before.ino());
}

#[test]
fn runtime_start_refuses_unsafe_records_before_creating_a_named_namespace() {
    let fixture = Fixture::new();
    let record = fixture.file(".podway/maintenance/runtime-reset.json", b"invalid record");
    let home = fixture.home();
    assert!(RuntimeResetLockV1::prepare_start(&home, &RuntimeModeV1::development()).is_err());
    assert!(!home.as_path().join("modes").exists());
    fs::remove_file(&record).unwrap();
    let victim = fixture.file("victim/record", b"untouched");
    symlink(&victim, &record).unwrap();
    assert!(RuntimeResetLockV1::prepare_start(&home, &RuntimeModeV1::development()).is_err());
    assert!(!home.as_path().join("modes").exists());
    assert_eq!(fs::read(victim).unwrap(), b"untouched");
}

#[test]
fn runtime_start_fence_applies_only_to_a_recorded_participant_mode() {
    let fixture = Fixture::new();
    let home = fixture.home();
    let root = fixture.directory(".podway/modes/dev");
    let account_root = home.as_path();
    let account_metadata = fs::metadata(account_root).unwrap();
    let root_metadata = fs::metadata(&root).unwrap();
    let mut fixtures: Value = serde_json::from_str(include_str!(
        "../../../tests/fixtures/v2/compatibility/runtime-reset-contract-reservation.json"
    ))
    .unwrap();
    let record = &mut fixtures["fixtures"]["podway.runtime-reset-record/v1"];
    record["plan"]["caller_uid"] = serde_json::json!(geteuid().as_raw());
    record["plan"]["account_root"] =
        serde_json::to_value(RuntimeResetPathV1::new(account_root).unwrap()).unwrap();
    record["plan"]["account_identity"] = serde_json::json!({
        "device": account_metadata.dev(), "inode": account_metadata.ino(),
        "uid": account_metadata.uid(), "mode": account_metadata.mode(),
        "links": account_metadata.nlink(),
    });
    record["plan"]["selection"] = serde_json::json!({"kind": "mode", "mode": "dev"});
    record["plan"]["targets"][0]["mode"] = serde_json::json!("dev");
    record["plan"]["targets"][0]["root"] =
        serde_json::to_value(RuntimeResetPathV1::new(&root).unwrap()).unwrap();
    record["plan"]["targets"][0]["root_identity"] = serde_json::json!({
        "device": root_metadata.dev(), "inode": root_metadata.ino(),
        "uid": root_metadata.uid(), "mode": root_metadata.mode(),
        "links": root_metadata.nlink(),
    });
    record["plan"]["targets"][0]["resource_classes"] = serde_json::json!([
        "registry",
        "runtime_recovery",
        "service_metadata",
        "socket",
        "daemon_log",
        "bootstrap_log",
        "state_directory",
        "logs_directory"
    ]);
    record["modes"][0]["mode"] = serde_json::json!("dev");
    let encoded = canonicalize_json_v1(&record["plan"]).unwrap();
    let token = format!(
        "{}.{:x}",
        encode_base64url_unpadded(encoded.as_bytes()),
        Sha256::digest(encoded.as_bytes())
    );
    record["token_sha256"] =
        serde_json::json!(format!("sha256:{:x}", Sha256::digest(token.as_bytes())));
    fixture.file(
        ".podway/maintenance/runtime-reset.json",
        &serde_json::to_vec(record).unwrap(),
    );

    let production = RuntimeResetLockV1::prepare_start(&home, &RuntimeModeV1::production());
    assert!(production.is_ok(), "{:?}", production.as_ref().err());
    drop(production);
    let development = RuntimeResetLockV1::prepare_start(&home, &RuntimeModeV1::development());
    assert!(matches!(development, Err(error) if error.code() == "RUNTIME_RESET_IN_PROGRESS"));
}

#[test]
fn production_lifecycle_refuses_an_unreadable_reset_record_before_side_effects() {
    struct MatchingContract;
    impl DaemonContractVerifierV1 for MatchingContract {
        fn verify(&self, _: &Path, _: &str, _: &str) -> Result<(), ServiceErrorV1> {
            Ok(())
        }
    }
    struct NoLaunchctl;
    impl LaunchctlRunnerV1 for NoLaunchctl {
        fn run(&self, _: &[String]) -> Result<LaunchctlOutputV1, ServiceErrorV1> {
            panic!("a fenced service command must not reach launchctl")
        }
    }
    let fixture = Fixture::new();
    fixture.namespace("prod");
    fixture.file(
        ".podway/maintenance/runtime-reset.json",
        b"invalid committed record",
    );
    drop(RuntimeResetLockV1::acquire_topology(&fixture.home()).unwrap());
    let paths = ServiceRuntimePathsV1::for_account_home(&fixture.0, geteuid().as_raw()).unwrap();
    let binary = fixture.file(
        "binary",
        &super::int_phase6_native_service::native_arm64_macho(0),
    );
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o700)).unwrap();
    let spec = InstallSpecV1::new(
        LocalPlatformPathV1::new(binary).unwrap(),
        ServiceLabelV1::podwayd(),
        paths.clone(),
        "podway",
        format!("sha256:{}", "0".repeat(64)),
    );
    let runner = MacosServiceCommandRunnerV1::new_with_contract_verifier(
        StdServiceFilesystemV1,
        NoLaunchctl,
        FixedServiceClockV1::new(now()),
        geteuid().as_raw(),
        MatchingContract,
    )
    .unwrap();
    let before = snapshot(&fixture.0);
    for command in [
        ServiceCommandV1::Install {
            requested_at: now(),
            spec: spec.clone(),
        },
        ServiceCommandV1::Update {
            requested_at: now(),
            spec,
        },
        ServiceCommandV1::Start {
            requested_at: now(),
            paths: paths.clone(),
        },
        ServiceCommandV1::Stop {
            requested_at: now(),
            paths: paths.clone(),
        },
        ServiceCommandV1::Restart {
            requested_at: now(),
            paths: paths.clone(),
        },
        ServiceCommandV1::Uninstall {
            requested_at: now(),
            paths: paths.clone(),
        },
        ServiceCommandV1::UninstallWithOptions {
            requested_at: now(),
            paths,
            options: UninstallOptionsV1::new(true),
        },
    ] {
        let operation = command.operation();
        let outcome = runner.run(command);
        assert!(
            matches!(&outcome, Err(ServiceErrorV1::OperationFailureV1 { operation: failed, source }) if *failed == operation && matches!(source.as_ref(), ServiceErrorV1::RuntimeResetV1(_))),
            "{operation}: {outcome:?}"
        );
        assert_eq!(snapshot(&fixture.0), before);
    }
}

#[test]
fn runtime_reset_locks_refuse_symlinks_hardlinks_and_public_permissions() {
    for case in ["symlink", "hardlink", "permissions"] {
        let fixture = Fixture::new();
        let victim = fixture.file("victim/file", b"untouched");
        let parent = fixture.directory(".podway/maintenance");
        let lock = parent.join("runtime-reset.lock");
        match case {
            "symlink" => symlink(&victim, &lock).unwrap(),
            "hardlink" => fs::hard_link(&victim, &lock).unwrap(),
            _ => {
                fs::write(&lock, b"").unwrap();
                fs::set_permissions(&lock, fs::Permissions::from_mode(0o644)).unwrap();
            }
        }
        assert!(
            RuntimeResetLockV1::acquire_reset(&fixture.home()).is_err(),
            "{case}"
        );
        assert_eq!(fs::read(victim).unwrap(), b"untouched");
    }
}

#[test]
fn concurrent_first_starts_share_one_stable_topology_anchor() {
    for _ in 0..16 {
        let fixture = Fixture::new();
        let home = fixture.home();
        let barrier = std::sync::Barrier::new(2);
        std::thread::scope(|scope| {
            let starts = [RuntimeModeV1::production(), RuntimeModeV1::development()].map(|mode| {
                let home = &home;
                let barrier = &barrier;
                scope.spawn(move || {
                    barrier.wait();
                    RuntimeResetLockV1::prepare_start(home, &mode).map(drop)
                })
            });
            for start in starts {
                start.join().unwrap().expect("concurrent first startup");
            }
        });
        assert!(fixture.0.join(".podway/run").is_dir());
        assert!(fixture.0.join(".podway/modes/dev/run").is_dir());
        let anchor = fixture.0.join(".podway/maintenance/runtime-start.lock");
        let inode = fs::metadata(&anchor).unwrap().ino();
        drop(RuntimeResetLockV1::acquire_topology(&home).unwrap());
        assert_eq!(fs::metadata(anchor).unwrap().ino(), inode);
    }
}

#[test]
fn topology_gate_serializes_namespace_creation_and_excludes_external_roots() {
    use std::{
        sync::{Arc, Barrier, mpsc},
        thread,
        time::Duration,
    };
    let fixture = Fixture::new();
    let home = fixture.home();
    let gate = RuntimeResetLockV1::acquire_topology(&home).unwrap();
    let ready = Arc::new(Barrier::new(2));
    let (sent, received) = mpsc::channel();
    let worker_home = home.clone();
    let worker_ready = Arc::clone(&ready);
    let worker = thread::spawn(move || {
        worker_ready.wait();
        let result = RuntimeResetLockV1::prepare_start(&worker_home, &RuntimeModeV1::development());
        sent.send(result.is_ok()).unwrap();
    });
    ready.wait();
    assert!(received.recv_timeout(Duration::from_millis(50)).is_err());
    assert!(!home.as_path().join("modes").exists());
    drop(gate);
    assert!(received.recv_timeout(Duration::from_secs(5)).unwrap());
    worker.join().unwrap();
    assert!(home.as_path().join("modes/dev/run").is_dir());
    let external = ServiceRuntimePathsV1::for_dev_home(
        &fixture.0,
        fixture.0.join("external"),
        geteuid().as_raw(),
    )
    .unwrap();
    assert!(external.ordinary_account_home().unwrap().is_none());
    assert!(
        StdServiceFilesystemV1
            .runtime_reset_gate(&external)
            .unwrap()
            .is_none()
    );
    assert!(!fixture.0.join("external").exists());
    let effective =
        ServiceRuntimePathsV1::for_effective_user_dev(Some(&fixture.0.join("external"))).unwrap();
    assert!(effective.ordinary_account_home().unwrap().is_none());
    assert_eq!(
        effective.runtime_directory().as_path(),
        fixture.0.join("external/run")
    );
}

#[test]
fn ordinary_dev_platform_aliases_keep_the_account_fence_and_reject_production() {
    let fixture = Fixture::new();
    let alias = Path::new("/tmp").join(fixture.0.strip_prefix("/private/tmp").unwrap());
    let paths = ServiceRuntimePathsV1::for_dev_home(
        &fixture.0,
        alias.join(".podway/modes/dev"),
        geteuid().as_raw(),
    )
    .unwrap();
    assert_eq!(
        paths.podway_home().unwrap().as_path(),
        fixture.0.join(".podway/modes/dev")
    );
    assert!(paths.ordinary_account_home().unwrap().is_some());
    fixture.file(
        ".podway/maintenance/runtime-reset.json",
        b"invalid committed record",
    );
    assert!(StdServiceFilesystemV1.runtime_reset_gate(&paths).is_err());
    assert!(!fixture.0.join(".podway/modes").exists());
    assert!(matches!(
        ServiceRuntimePathsV1::for_dev_home(&fixture.0, alias.join(".podway"), geteuid().as_raw()),
        Err(ServicePathErrorV1::DevHomeConflictsProduction { .. })
    ));
}

#[test]
fn ordinary_dev_symlink_alias_keeps_the_account_fence_and_rejects_production() {
    let fixture = Fixture::new();
    fixture.directory(".podway/modes/dev");
    let alias = fixture.0.join("account-alias");
    symlink(&fixture.0, &alias).unwrap();
    let paths = ServiceRuntimePathsV1::for_dev_home(
        &fixture.0,
        alias.join(".podway/modes/dev"),
        geteuid().as_raw(),
    )
    .unwrap();
    assert_eq!(
        paths.podway_home().unwrap().as_path(),
        fixture.0.join(".podway/modes/dev")
    );
    assert!(paths.ordinary_account_home().unwrap().is_some());
    fixture.file(
        ".podway/maintenance/runtime-reset.json",
        b"invalid committed record",
    );
    assert!(StdServiceFilesystemV1.runtime_reset_gate(&paths).is_err());
    assert!(matches!(
        ServiceRuntimePathsV1::for_dev_home(&fixture.0, alias.join(".podway"), geteuid().as_raw()),
        Err(ServicePathErrorV1::DevHomeConflictsProduction { .. })
    ));
}
fn snapshot(root: &Path) -> Vec<(PathBuf, u64, u32, Vec<u8>)> {
    fn walk(path: &Path, result: &mut Vec<(PathBuf, u64, u32, Vec<u8>)>) {
        let metadata = fs::symlink_metadata(path).unwrap();
        let bytes = if metadata.is_file() {
            fs::read(path).unwrap()
        } else {
            Vec::new()
        };
        result.push((path.to_path_buf(), metadata.ino(), metadata.mode(), bytes));
        if metadata.is_dir() {
            for entry in fs::read_dir(path).unwrap() {
                walk(&entry.unwrap().path(), result);
            }
        }
    }
    let mut result = Vec::new();
    walk(root, &mut result);
    result.sort();
    result
}

#[test]
fn absent_and_retired_modes_create_nothing() {
    let fixture = Fixture::new();
    let before = snapshot(&fixture.0);
    let plan = fixture
        .planner()
        .plan(RuntimeResetSelectionV1::AllModes, now())
        .unwrap();
    assert_eq!(plan.status, RuntimeResetPlanStatusV1::NoChange);
    assert!(plan.plan_token.is_none());
    assert_eq!(plan.targets.len(), 1);
    assert_eq!(snapshot(&fixture.0), before);
    fixture.file(".podway/modes/dev/run/podwayd.lock", b"");
    fixture.file(".podway/modes/dev/state/workspaces.json.lock", b"");
    let before = snapshot(&fixture.0);
    let plan = fixture.planner().plan(mode("dev"), now()).unwrap();
    assert_eq!(plan.status, RuntimeResetPlanStatusV1::NoChange);
    assert!(
        plan.preserved
            .iter()
            .any(|item| item.path.display.ends_with("workspaces.json.lock"))
    );
    assert_eq!(snapshot(&fixture.0), before);
}

#[test]
fn corrupt_registry_plan_is_read_only_and_tokens_bind_identity_and_expiry() {
    let fixture = Fixture::new();
    let root = fixture.namespace("dev");
    let before = snapshot(&fixture.0);
    let mut planner = fixture.planner();
    let plan = planner.plan(mode("dev"), now()).unwrap();
    assert_eq!(plan.status, RuntimeResetPlanStatusV1::Ready);
    assert_eq!(plan.targets[0].state, RuntimeResetTargetStateV1::Offline);
    let token = plan.plan_token.unwrap();
    assert_eq!(plan.expires_at_ms, Some(601000));
    planner
        .validate_precommit_token(&token, &mode("dev"), UnixMillis::new(2000))
        .unwrap();
    assert_eq!(snapshot(&fixture.0), before);
    fs::write(
        root.join("state/workspaces.json"),
        b"new registry preview bytes",
    )
    .unwrap();
    planner
        .validate_precommit_token(&token, &mode("dev"), UnixMillis::new(2000))
        .unwrap();
    for (selection, time) in [
        (mode("qa"), now()),
        (mode("dev"), UnixMillis::new(601000)),
        (mode("dev"), UnixMillis::new(999)),
    ] {
        assert_eq!(
            planner
                .validate_precommit_token(&token, &selection, time)
                .unwrap_err()
                .code(),
            "RUNTIME_RESET_PLAN_STALE"
        );
    }
    let old = root.join("run/podwayd.lock");
    fs::rename(&old, fixture.0.join("old-lock")).unwrap();
    fixture.file(".podway/modes/dev/run/podwayd.lock", b"");
    assert_eq!(
        planner
            .validate_precommit_token(&token, &mode("dev"), now())
            .unwrap_err()
            .code(),
        "RUNTIME_RESET_PLAN_STALE"
    );
}

#[test]
fn all_mode_inventory_is_sorted_and_new_modes_stale_the_token() {
    let fixture = Fixture::new();
    fixture.namespace("zeta");
    fixture.namespace("alpha");
    let mut planner = fixture.planner();
    let plan = planner
        .plan(RuntimeResetSelectionV1::AllModes, now())
        .unwrap();
    assert_eq!(
        plan.targets
            .iter()
            .map(|target| target.mode.as_str())
            .collect::<Vec<_>>(),
        ["prod", "alpha", "zeta"]
    );
    let token = plan.plan_token.unwrap();
    fixture.directory(".podway/modes/new-mode");
    assert_eq!(
        planner
            .validate_precommit_token(&token, &RuntimeResetSelectionV1::AllModes, now())
            .unwrap_err()
            .code(),
        "RUNTIME_RESET_PLAN_STALE"
    );
}

#[test]
fn malicious_layouts_never_produce_tokens_or_follow_links() {
    for attack in 0..7 {
        let fixture = Fixture::new();
        let root = fixture.namespace("dev");
        match attack {
            0 => {
                fs::rename(root.join("state"), fixture.0.join("outside")).unwrap();
                symlink(fixture.0.join("outside"), root.join("state")).unwrap();
            }
            1 => {
                fs::hard_link(
                    root.join("state/workspaces.json"),
                    fixture.0.join("hard-link"),
                )
                .unwrap();
            }
            2 => {
                fs::set_permissions(
                    root.join("state/workspaces.json"),
                    fs::Permissions::from_mode(0o644),
                )
                .unwrap();
            }
            3 => {
                fixture.file(".podway/modes/dev/state/unrelated", b"keep");
            }
            4 => {
                fs::remove_file(root.join("run/podwayd.lock")).unwrap();
            }
            5 => {
                fs::set_permissions(root.join("state"), fs::Permissions::from_mode(0o755)).unwrap();
            }
            6 => {
                symlink("missing", root.join("state/recovery.json")).unwrap();
            }
            _ => unreachable!(),
        }
        let before = snapshot(&fixture.0);
        let plan = fixture.planner().plan(mode("dev"), now()).unwrap();
        assert_eq!(
            plan.status,
            RuntimeResetPlanStatusV1::Blocked,
            "attack {attack}"
        );
        assert!(plan.plan_token.is_none());
        assert_eq!(snapshot(&fixture.0), before);
    }
}

#[test]
fn held_locks_and_unknown_peers_block_but_valid_idle_generations_bind() {
    let fixture = Fixture::new();
    let root = fixture.namespace("dev");
    let lock = Flock::lock(
        File::open(root.join("run/podwayd.lock")).unwrap(),
        FlockArg::LockExclusiveNonblock,
    )
    .unwrap();
    let plan = fixture.planner().plan(mode("dev"), now()).unwrap();
    assert_eq!(plan.targets[0].reason, Some(RuntimeResetReasonV1::LockHeld));
    let process = RuntimeResetProcessV1 {
        pid: std::process::id(),
        process_id: uuid::Uuid::new_v4().to_string(),
        executable: RuntimeResetPathV1::new(&std::env::current_exe().unwrap()).unwrap(),
        started_at_ms: 1,
    };
    for (peer, state) in [
        (
            RuntimeResetPeerV1::Unsupported,
            RuntimeResetTargetStateV1::Unsupported,
        ),
        (
            RuntimeResetPeerV1::Live {
                process: process.clone(),
                busy_reason: Some(RuntimeResetReasonV1::Activity),
            },
            RuntimeResetTargetStateV1::Busy,
        ),
        (
            RuntimeResetPeerV1::Live {
                process,
                busy_reason: None,
            },
            RuntimeResetTargetStateV1::LiveIdle,
        ),
    ] {
        let mut planner =
            RuntimeResetPlannerV1::new(fixture.home(), Launchctl(false), Inspector(peer));
        let plan = planner.plan(mode("dev"), now()).unwrap();
        assert_eq!(plan.targets[0].state, state);
        assert_eq!(
            plan.plan_token.is_some(),
            state == RuntimeResetTargetStateV1::LiveIdle
        );
    }
    drop(lock);
}

#[test]
fn resource_and_mode_bounds_fail_without_truncation() {
    let fixture = Fixture::new();
    fixture.namespace("dev");
    let state = fixture.0.join(".podway/modes/dev/state");
    for index in 0..4096 {
        fixture.file(
            &format!(".podway/modes/dev/state/.podway-registry-v1-1-{index}.tmp"),
            b"",
        );
    }
    assert_eq!(
        fixture
            .planner()
            .plan(mode("dev"), now())
            .unwrap_err()
            .code(),
        "RUNTIME_RESET_LIMIT_EXCEEDED"
    );
    fs::remove_dir_all(state).unwrap();
    for index in 0..64 {
        fixture.directory(&format!(".podway/modes/m{index}"));
    }
    assert_eq!(
        fixture
            .planner()
            .plan(RuntimeResetSelectionV1::AllModes, now())
            .unwrap_err()
            .code(),
        "RUNTIME_RESET_LIMIT_EXCEEDED"
    );
    assert_eq!(
        fixture
            .planner()
            .validate_precommit_token(&"A".repeat(262145), &mode("dev"), now())
            .unwrap_err()
            .code(),
        "RUNTIME_RESET_LIMIT_EXCEEDED"
    );
    for index in 0..257 {
        fixture.directory(&format!(".podway/modes/.entry-{index}"));
    }
    assert_eq!(
        fixture
            .planner()
            .plan(RuntimeResetSelectionV1::AllModes, now())
            .unwrap_err()
            .code(),
        "RUNTIME_RESET_LIMIT_EXCEEDED"
    );
}

#[test]
fn registry_anchor_replacement_invalidates_a_plan() {
    let fixture = Fixture::new();
    fixture.namespace("dev");
    let anchor = fixture.file(".podway/modes/dev/state/workspaces.json.lock", b"");
    let mut planner = fixture.planner();
    let plan = planner.plan(mode("dev"), now()).unwrap();
    fs::rename(&anchor, fixture.0.join("old-registry-lock")).unwrap();
    fixture.file(".podway/modes/dev/state/workspaces.json.lock", b"");
    assert_eq!(
        planner
            .validate_precommit_token(plan.plan_token.as_deref().unwrap(), &mode("dev"), now())
            .unwrap_err()
            .code(),
        "RUNTIME_RESET_PLAN_STALE"
    );
}

#[test]
fn managed_namespaces_are_excluded_without_traversing_external_paths() {
    let fixture = Fixture::new();
    let root = fixture.namespace("dev");
    let paths = ServiceRuntimePathsV1::for_runtime_root(
        &root,
        RuntimeModeV1::development(),
        geteuid().as_raw(),
    )
    .unwrap();
    let executable = serde_json::json!({"path": fixture.0.join("absent-executable"), "sha256": format!("sha256:{}", "a".repeat(64))});
    let marker = serde_json::json!({
        "schema": "podway.managed-runtime/v3", "metadata_version": 3, "purpose": "aquarium-development",
        "euid": geteuid().as_raw(), "canonical_root": root, "mode": "dev",
        "paths": {
            "lock": paths.global_lock_path().as_path(), "socket": paths.socket_path().as_path(),
            "service_state": paths.metadata_index_path().as_path(), "registry": paths.workspace_registry_path().as_path(),
            "recovery": paths.recovery_path().as_path(), "log": paths.log_path().as_path(),
            "bootstrap_log": paths.bootstrap_log_path().as_path(),
        },
        "sandbox_root": null, "executables": {"cli": executable, "daemon": executable, "controller": executable},
        "generation": "a".repeat(40),
    });
    let metadata = fixture.file(
        ".podway/modes/dev/runtime.json",
        &serde_json::to_vec(&marker).unwrap(),
    );
    // Exclusion must happen before opening an untrusted descendant.
    symlink("/missing-external-resource", root.join("logs")).unwrap();
    let before = snapshot(&fixture.0);
    for selection in [mode("dev"), RuntimeResetSelectionV1::AllModes] {
        let plan = fixture.planner().plan(selection, now()).unwrap();
        assert_eq!(plan.status, RuntimeResetPlanStatusV1::NoChange);
        assert_eq!(plan.excluded.len(), 2);
        assert!(
            plan.targets
                .iter()
                .all(|target| target.mode.is_production())
        );
    }
    assert_eq!(snapshot(&fixture.0), before);
    fs::write(metadata, b"{invalid}").unwrap();
    let plan = fixture.planner().plan(mode("dev"), now()).unwrap();
    assert_eq!(plan.status, RuntimeResetPlanStatusV1::Blocked);
    assert!(plan.plan_token.is_none());
}

#[test]
fn loaded_production_service_cannot_be_mistaken_for_offline() {
    let fixture = Fixture::new();
    fixture.namespace("prod");
    let mut planner = RuntimeResetPlannerV1::new(
        fixture.home(),
        Launchctl(true),
        Inspector(RuntimeResetPeerV1::Offline),
    );
    let plan = planner.plan(mode("prod"), now()).unwrap();
    assert_eq!(plan.status, RuntimeResetPlanStatusV1::Blocked);
    assert_eq!(
        plan.targets[0].reason,
        Some(RuntimeResetReasonV1::ServiceLoaded)
    );
}

#[test]
fn live_identity_without_a_held_singleton_never_gets_a_token() {
    let fixture = Fixture::new();
    fixture.namespace("dev");
    let mut planner = RuntimeResetPlannerV1::new(
        fixture.home(),
        Launchctl(false),
        Inspector(RuntimeResetPeerV1::Live {
            process: RuntimeResetProcessV1 {
                pid: std::process::id(),
                process_id: uuid::Uuid::new_v4().to_string(),
                executable: RuntimeResetPathV1::new(&std::env::current_exe().unwrap()).unwrap(),
                started_at_ms: 1,
            },
            busy_reason: None,
        }),
    );
    let before = snapshot(&fixture.0);
    let plan = planner.plan(mode("dev"), now()).unwrap();
    assert_eq!(plan.status, RuntimeResetPlanStatusV1::Blocked);
    assert_eq!(plan.targets[0].state, RuntimeResetTargetStateV1::Unsafe);
    assert_eq!(
        plan.targets[0].reason,
        Some(RuntimeResetReasonV1::IdentityChanged)
    );
    assert!(plan.plan_token.is_none());
    assert_eq!(snapshot(&fixture.0), before);
}

#[test]
fn production_plist_is_inventoried_and_its_replacement_stales_the_token() {
    let fixture = Fixture::new();
    fixture.namespace("prod");
    let plist = fixture.file(
        "Library/LaunchAgents/dev.podway.podwayd.plist",
        b"owned plist",
    );
    let mut planner = fixture.planner();
    let before = snapshot(&fixture.0);
    let plan = planner.plan(mode("prod"), now()).unwrap();
    assert_eq!(plan.status, RuntimeResetPlanStatusV1::Ready);
    assert!(plan.targets[0].resources.iter().any(|resource| {
        resource.class == RuntimeResetResourceClassV1::ServicePlist
            && resource.path == RuntimeResetPathV1::new(&plist).unwrap()
    }));
    let token = plan.plan_token.unwrap();
    planner
        .validate_precommit_token(&token, &mode("prod"), now())
        .unwrap();
    assert_eq!(snapshot(&fixture.0), before);
    fs::rename(&plist, fixture.0.join("old-plist")).unwrap();
    fixture.file(
        "Library/LaunchAgents/dev.podway.podwayd.plist",
        b"owned plist",
    );
    assert_eq!(
        planner
            .validate_precommit_token(&token, &mode("prod"), now())
            .unwrap_err()
            .code(),
        "RUNTIME_RESET_PLAN_STALE"
    );
}

#[test]
fn invalid_mode_inventory_aborts_without_a_partial_plan() {
    for name in ["invalid!", "prod"] {
        let fixture = Fixture::new();
        fixture.namespace("dev");
        fixture.directory(&format!(".podway/modes/{name}"));
        let before = snapshot(&fixture.0);
        let error = fixture
            .planner()
            .plan(RuntimeResetSelectionV1::AllModes, now())
            .unwrap_err();
        assert_eq!(error.code(), "RUNTIME_RESET_UNSAFE");
        assert_eq!(error.reason, RuntimeResetReasonV1::UnexpectedEntry);
        assert_eq!(snapshot(&fixture.0), before);
    }
}

#[test]
fn vanished_namespace_stales_a_precommit_token() {
    let fixture = Fixture::new();
    let root = fixture.namespace("dev");
    let mut planner = fixture.planner();
    let token = planner
        .plan(mode("dev"), now())
        .unwrap()
        .plan_token
        .unwrap();
    fs::remove_dir_all(root).unwrap();
    let error = planner
        .validate_precommit_token(&token, &mode("dev"), now())
        .unwrap_err();
    assert_eq!(error.code(), "RUNTIME_RESET_PLAN_STALE");
    assert_eq!(error.reason, RuntimeResetReasonV1::IdentityChanged);
}

#[test]
fn ambiguous_launchctl_output_never_proves_an_offline_service() {
    struct AmbiguousLaunchctl(i32);
    impl LaunchctlRunnerV1 for AmbiguousLaunchctl {
        fn run(&self, _: &[String]) -> Result<LaunchctlOutputV1, ServiceErrorV1> {
            Ok(LaunchctlOutputV1 {
                exit_status: self.0,
                stdout: "unrecognized service state".to_owned(),
                stderr: "truncated diagnostic".to_owned(),
            })
        }
    }
    let fixture = Fixture::new();
    fixture.namespace("prod");
    for exit_status in [0, 113, 1] {
        let mut planner = RuntimeResetPlannerV1::new(
            fixture.home(),
            AmbiguousLaunchctl(exit_status),
            Inspector(RuntimeResetPeerV1::Offline),
        );
        let before = snapshot(&fixture.0);
        let plan = planner.plan(mode("prod"), now()).unwrap();
        assert_eq!(plan.status, RuntimeResetPlanStatusV1::Blocked);
        assert_eq!(plan.targets[0].state, RuntimeResetTargetStateV1::Unsafe);
        assert_eq!(
            plan.targets[0].reason,
            Some(RuntimeResetReasonV1::ServiceLoaded)
        );
        assert!(plan.plan_token.is_none());
        assert_eq!(snapshot(&fixture.0), before);
    }
}

#[test]
fn orphaned_production_plist_blocks_without_creating_a_namespace() {
    let fixture = Fixture::new();
    let plist = fixture.file(
        "Library/LaunchAgents/dev.podway.podwayd.plist",
        b"owned plist",
    );
    let before = snapshot(&fixture.0);
    let plan = fixture.planner().plan(mode("prod"), now()).unwrap();
    assert_eq!(plan.status, RuntimeResetPlanStatusV1::Blocked);
    assert!(plan.plan_token.is_none());
    assert_eq!(plan.targets.len(), 1);
    assert_eq!(plan.targets[0].state, RuntimeResetTargetStateV1::Unsafe);
    assert_eq!(
        plan.targets[0].reason,
        Some(RuntimeResetReasonV1::UnsafePath)
    );
    assert!(plan.targets[0].resources.iter().any(|resource| {
        resource.class == RuntimeResetResourceClassV1::ServicePlist
            && resource.path == RuntimeResetPathV1::new(&plist).unwrap()
    }));
    assert!(!fixture.0.join(".podway").exists());
    assert_eq!(snapshot(&fixture.0), before);
}

#[test]
fn mixed_all_mode_readiness_withholds_the_token_and_retains_inventory() {
    let fixture = Fixture::new();
    fixture.namespace("dev");
    let busy_root = fixture.namespace("stage");
    let lock = Flock::lock(
        File::open(busy_root.join("run/podwayd.lock")).unwrap(),
        FlockArg::LockExclusiveNonblock,
    )
    .unwrap();
    let before = snapshot(&fixture.0);
    let plan = fixture
        .planner()
        .plan(RuntimeResetSelectionV1::AllModes, now())
        .unwrap();
    assert_eq!(plan.status, RuntimeResetPlanStatusV1::Blocked);
    assert!(plan.plan_token.is_none());
    let ready = plan
        .targets
        .iter()
        .find(|target| target.mode.as_str() == "dev")
        .unwrap();
    assert_eq!(ready.state, RuntimeResetTargetStateV1::Offline);
    assert!(
        ready
            .resources
            .iter()
            .any(|resource| { resource.class == RuntimeResetResourceClassV1::Registry })
    );
    let blocked = plan
        .targets
        .iter()
        .find(|target| target.mode.as_str() == "stage")
        .unwrap();
    assert_eq!(blocked.state, RuntimeResetTargetStateV1::Unsafe);
    assert_eq!(blocked.reason, Some(RuntimeResetReasonV1::LockHeld));
    assert_eq!(snapshot(&fixture.0), before);
    drop(lock);
}

#[test]
fn launchctl_execution_failure_never_proves_an_offline_service() {
    struct FailedLaunchctl;
    impl LaunchctlRunnerV1 for FailedLaunchctl {
        fn run(&self, _: &[String]) -> Result<LaunchctlOutputV1, ServiceErrorV1> {
            Err(ServiceErrorV1::IoV1 {
                operation: None,
                message: "injected launchctl spawn failure".to_owned(),
            })
        }
    }
    let fixture = Fixture::new();
    fixture.namespace("prod");
    let mut planner = RuntimeResetPlannerV1::new(
        fixture.home(),
        FailedLaunchctl,
        Inspector(RuntimeResetPeerV1::Offline),
    );
    let before = snapshot(&fixture.0);
    let plan = planner.plan(mode("prod"), now()).unwrap();
    assert_eq!(plan.status, RuntimeResetPlanStatusV1::Blocked);
    assert!(plan.plan_token.is_none());
    assert_eq!(plan.targets[0].state, RuntimeResetTargetStateV1::Unsafe);
    assert_eq!(plan.targets[0].reason, Some(RuntimeResetReasonV1::Io));
    assert_eq!(snapshot(&fixture.0), before);
}

#[test]
fn logs_inventory_accepts_only_owned_rotation_names_without_mutation() {
    let fixture = Fixture::new();
    fixture.namespace("dev");
    let names = [
        ("podwayd.log", RuntimeResetResourceClassV1::DaemonLog),
        ("podwayd.log.1", RuntimeResetResourceClassV1::DaemonLog),
        ("podwayd.log.9", RuntimeResetResourceClassV1::DaemonLog),
        (
            "podwayd-bootstrap.log",
            RuntimeResetResourceClassV1::BootstrapLog,
        ),
        (
            "podwayd-bootstrap.log.4",
            RuntimeResetResourceClassV1::BootstrapLog,
        ),
    ];
    for (name, _) in &names {
        fixture.file(&format!(".podway/modes/dev/logs/{name}"), b"retained log");
    }
    let before = snapshot(&fixture.0);
    let plan = fixture.planner().plan(mode("dev"), now()).unwrap();
    assert_eq!(plan.status, RuntimeResetPlanStatusV1::Ready);
    let resources = &plan.targets[0].resources;
    for (name, class) in names {
        let expected =
            RuntimeResetPathV1::new(&fixture.0.join(format!(".podway/modes/dev/logs/{name}")))
                .unwrap();
        assert!(
            resources
                .iter()
                .any(|resource| resource.path == expected && resource.class == class)
        );
    }
    assert_eq!(
        resources
            .iter()
            .filter(|resource| resource.class == RuntimeResetResourceClassV1::LogsDirectory)
            .count(),
        1
    );
    assert_eq!(snapshot(&fixture.0), before);
    for name in [
        "podwayd.log.10",
        "podwayd.log.01",
        "podwayd.log.0",
        "podwayd-bootstrap.log.5",
        "unrelated.log",
    ] {
        let path = fixture.file(&format!(".podway/modes/dev/logs/{name}"), b"do not remove");
        let before = snapshot(&fixture.0);
        let plan = fixture.planner().plan(mode("dev"), now()).unwrap();
        assert_eq!(plan.status, RuntimeResetPlanStatusV1::Blocked, "{name}");
        assert_eq!(
            plan.targets[0].reason,
            Some(RuntimeResetReasonV1::UnexpectedEntry),
            "{name}"
        );
        assert!(plan.plan_token.is_none());
        assert_eq!(snapshot(&fixture.0), before);
        fs::remove_file(path).unwrap();
    }
}

#[test]
fn service_metadata_inventory_binds_content_and_inode_without_parsing() {
    let fixture = Fixture::new();
    fixture.namespace("prod");
    let metadata = fixture.file(".podway/state/service.json", b"corrupt metadata");
    let temporary = fixture.file(
        ".podway/state/.service.json.1.2.1.3.tmp",
        b"partial metadata",
    );
    let mut planner = fixture.planner();
    let before = snapshot(&fixture.0);
    let plan = planner.plan(mode("prod"), now()).unwrap();
    assert_eq!(plan.status, RuntimeResetPlanStatusV1::Ready);
    for path in [&metadata, &temporary] {
        assert!(
            plan.targets[0]
                .resources
                .iter()
                .any(
                    |resource| resource.class == RuntimeResetResourceClassV1::ServiceMetadata
                        && resource.path == RuntimeResetPathV1::new(path).unwrap()
                )
        );
    }
    let token = plan.plan_token.unwrap();
    planner
        .validate_precommit_token(&token, &mode("prod"), now())
        .unwrap();
    assert_eq!(snapshot(&fixture.0), before);
    // Same inode, different bytes: content binding must independently stale the plan.
    fs::write(&metadata, b"changed metadata").unwrap();
    assert_eq!(
        planner
            .validate_precommit_token(&token, &mode("prod"), now())
            .unwrap_err()
            .code(),
        "RUNTIME_RESET_PLAN_STALE"
    );
    let token = planner
        .plan(mode("prod"), now())
        .unwrap()
        .plan_token
        .unwrap();
    // Same bytes, different inode: content equality cannot revive the old plan.
    fs::rename(&metadata, fixture.0.join("old-service-metadata")).unwrap();
    fixture.file(".podway/state/service.json", b"changed metadata");
    assert_eq!(
        planner
            .validate_precommit_token(&token, &mode("prod"), now())
            .unwrap_err()
            .code(),
        "RUNTIME_RESET_PLAN_STALE"
    );
    let invalid = fixture.file(
        ".podway/state/.service.json.1.2.9.3.tmp",
        b"unrecognized temporary",
    );
    let before = snapshot(&fixture.0);
    let plan = planner.plan(mode("prod"), now()).unwrap();
    assert_eq!(plan.status, RuntimeResetPlanStatusV1::Blocked);
    assert_eq!(
        plan.targets[0].reason,
        Some(RuntimeResetReasonV1::UnexpectedEntry)
    );
    assert!(invalid.exists());
    assert_eq!(snapshot(&fixture.0), before);
}

#[test]
fn preserved_inventory_limit_rejects_complete_selection_without_truncation() {
    let fixture = Fixture::new();
    // Five preserved anchors/directories per namespace exceeds 256 below the mode bound.
    for index in 0..52 {
        fixture.namespace(&format!("mode-{index:02}"));
        fixture.file(
            &format!(".podway/modes/mode-{index:02}/state/workspaces.json.lock"),
            b"",
        );
    }
    let before = snapshot(&fixture.0);
    let error = fixture
        .planner()
        .plan(RuntimeResetSelectionV1::AllModes, now())
        .unwrap_err();
    assert_eq!(error.code(), "RUNTIME_RESET_LIMIT_EXCEEDED");
    assert_eq!(error.reason, RuntimeResetReasonV1::BoundExceeded);
    assert_eq!(snapshot(&fixture.0), before);
}
