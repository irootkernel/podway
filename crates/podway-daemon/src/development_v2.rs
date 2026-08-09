//! Development-only Procedure v2 admission provenance.
//!
//! This module does not execute a Procedure v2 command. It proves that a future handler is
//! running inside the one helper-managed disposable runtime and that the selected workspace is
//! the exact sandbox named by that runtime. The accepting constructor is compiled only for an
//! explicitly featured debug build; every ordinary and release build is structurally closed.

use std::path::{Path, PathBuf};

#[cfg(all(feature = "development-v2-admission", debug_assertions))]
use nix::{
    fcntl::{OFlag, open},
    sys::stat::Mode,
};
#[cfg(all(feature = "development-v2-admission", debug_assertions))]
use podway_service::SERVICE_DAEMON_BINARY_MAX_BYTES_V1;
use podway_service::ServiceRuntimePathsV1;
use podway_store::WorkspaceBindingV1;
#[cfg(all(feature = "development-v2-admission", debug_assertions))]
use std::{fs::File, io::Read as _};

use crate::{registry::load_registry_readonly_v1, workspace::ValidatedRuntimeDirectoryV1};

pub(crate) const DEVELOPMENT_V2_ADMISSION_FEATURE_V1: &str = "development-v2-admission";
pub(crate) const DEVELOPMENT_V2_MARKER_SCHEMA_V1: &str =
    "podway.disposable-development-workspace/v1";

#[derive(Clone, Debug, Default)]
pub(crate) struct DevelopmentV2AdmissionGateV1 {
    enabled: Option<DevelopmentV2ProcessIdentityV1>,
}

#[derive(Clone, Debug)]
struct DevelopmentV2ProcessIdentityV1 {
    managed_root: PathBuf,
    account_root: PathBuf,
    dev_home: PathBuf,
    sandbox: PathBuf,
    socket_path: PathBuf,
    state_directory: PathBuf,
    daemon_path: PathBuf,
    daemon_sha256: String,
    production_paths: ServiceRuntimePathsV1,
    uid: u32,
}

impl DevelopmentV2AdmissionGateV1 {
    pub(crate) const fn closed() -> Self {
        Self { enabled: None }
    }

    /// Captures immutable process topology at daemon startup. Invalid or incomplete development
    /// provenance deliberately leaves the daemon usable for v1 while keeping v2 admission closed.
    pub(crate) fn from_process(
        dev_mode: bool,
        active_paths: &ServiceRuntimePathsV1,
        current_executable: &Path,
    ) -> Self {
        #[cfg(all(feature = "development-v2-admission", debug_assertions))]
        {
            Self {
                enabled: enabled_process_identity_v1(dev_mode, active_paths, current_executable),
            }
        }
        #[cfg(not(all(feature = "development-v2-admission", debug_assertions)))]
        {
            let _ = (dev_mode, active_paths, current_executable);
            Self::closed()
        }
    }

    pub(crate) fn process_is_eligible(&self) -> bool {
        self.enabled.is_some()
    }

    /// Revalidates directory provenance, the helper-issued marker, and normal-registry exclusion
    /// from a read-only two-pass workspace resolution. It grants no scheduler or Store mutation
    /// authority.
    pub(crate) fn permits_workspace(
        &self,
        binding: &WorkspaceBindingV1,
        worktree: &podway_git::ValidatedWorktreeV1,
    ) -> bool {
        let Some(identity) = self.enabled.as_ref() else {
            return false;
        };
        let workspace_root = binding.last_validated_root().to_path_buf();
        if workspace_root != identity.sandbox {
            return false;
        }
        if !validate_request_directories_v1(identity)
            || !marker_matches_identity_v1(identity, binding, worktree)
        {
            return false;
        }
        production_registry_excludes_v1(&identity.production_paths, binding)
    }
}

fn marker_matches_identity_v1(
    identity: &DevelopmentV2ProcessIdentityV1,
    binding: &WorkspaceBindingV1,
    worktree: &podway_git::ValidatedWorktreeV1,
) -> bool {
    if binding.last_validated_root().to_path_buf() != identity.sandbox {
        return false;
    }
    let Ok(runtime) = ValidatedRuntimeDirectoryV1::open(worktree) else {
        return false;
    };
    let Ok(Some(bytes)) = runtime.read_development_v2_marker_bytes() else {
        return false;
    };
    if podway_core::verify_canonical_json_v1(&bytes).is_err() {
        return false;
    }
    let Ok(marker) = serde_json::from_slice::<DevelopmentV2MarkerV1>(&bytes) else {
        return false;
    };
    marker.schema == DEVELOPMENT_V2_MARKER_SCHEMA_V1
        && marker.feature == DEVELOPMENT_V2_ADMISSION_FEATURE_V1
        && marker.uid == identity.uid
        && marker.managed_root == identity.managed_root
        && marker.account_root == identity.account_root
        && marker.dev_home == identity.dev_home
        && marker.workspace_root == identity.sandbox
        && marker.socket_path == identity.socket_path
        && marker.state_directory == identity.state_directory
        && marker.daemon_path == identity.daemon_path
        && marker.daemon_sha256 == identity.daemon_sha256
}

#[cfg(all(feature = "development-v2-admission", debug_assertions))]
const DEVELOPMENT_V2_METADATA_MAX_BYTES_V1: u64 = 16 * 1024;
#[cfg(all(feature = "development-v2-admission", debug_assertions))]
const PRIVATE_FILE_MODE_V1: u32 = 0o600;
#[cfg(all(feature = "development-v2-admission", debug_assertions))]
const EXECUTABLE_FILE_MODE_V1: u32 = 0o755;
const PRIVATE_DIRECTORY_MODE_V1: u32 = 0o700;

#[cfg(all(feature = "development-v2-admission", debug_assertions))]
fn enabled_process_identity_v1(
    dev_mode: bool,
    active_paths: &ServiceRuntimePathsV1,
    current_executable: &Path,
) -> Option<DevelopmentV2ProcessIdentityV1> {
    use std::os::unix::ffi::OsStrExt as _;

    use sha2::{Digest as _, Sha256};

    if !dev_mode {
        return None;
    }
    let dev_home = active_paths.podway_home()?.as_path().to_path_buf();
    let managed_root = dev_home.parent()?.to_path_buf();
    if managed_root.parent()? != Path::new("/private/tmp")
        || !managed_root
            .file_name()?
            .to_string_lossy()
            .starts_with("podway-dev-")
    {
        return None;
    }
    let metadata_path = managed_root.join("runtime.json");
    let uid = nix::unistd::geteuid().as_raw();
    let metadata = read_private_json_v1::<DevelopmentRuntimeMetadataV1>(
        &metadata_path,
        uid,
        DEVELOPMENT_V2_METADATA_MAX_BYTES_V1,
    )?;
    let checkout = metadata.checkout.canonicalize().ok()?;
    let checkout_digest = format!("{:x}", Sha256::digest(checkout.as_os_str().as_bytes()));
    let expected_managed_root =
        Path::new("/private/tmp").join(format!("podway-dev-{uid}-{}", &checkout_digest[..12]));
    let current_executable = current_executable.canonicalize().ok()?;
    let daemon_path = metadata.snapshot.podwayd.canonicalize().ok()?;
    let production_paths = ServiceRuntimePathsV1::for_effective_user().ok()?;
    let state_directory = active_paths.workspace_registry_path().as_path().parent()?;
    let account_lock = Path::new(&metadata.account_root).join(".podway/run/podwayd.lock");
    if metadata.schema != "podway.dev-runtime/v1"
        || metadata.uid != uid
        || metadata.root != managed_root
        || metadata.dev_home != dev_home
        || metadata.sandbox != managed_root.join("sandbox")
        || metadata.account_root != managed_root.join("account")
        || metadata.checkout != checkout
        || managed_root != expected_managed_root
        || metadata.snapshot.directory != managed_root.join("snapshots").join(&metadata.snapshot.id)
        || metadata.snapshot.podway != metadata.snapshot.directory.join("podway")
        || metadata.snapshot.podwayd != metadata.snapshot.directory.join("podwayd")
        || metadata.snapshot.podwayd != daemon_path
        || metadata.snapshot.directory.file_name()?.to_string_lossy() != metadata.snapshot.id
        || metadata.snapshot.podway_sha256.len() != 64
        || !metadata
            .snapshot
            .podway_sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        || active_paths.global_lock_path().as_path() != account_lock
        || active_paths.socket_path().as_path() != dev_home.join("run/podwayd.sock")
        || active_paths.workspace_registry_path().as_path()
            != dev_home.join("state/workspaces.json")
        || current_executable != daemon_path
        || !daemon_path.starts_with(managed_root.join("snapshots"))
        || active_paths.socket_path().as_path() == production_paths.socket_path().as_path()
        || active_paths.workspace_registry_path().as_path()
            == production_paths.workspace_registry_path().as_path()
        || active_paths.podway_home().map(|path| path.as_path())
            == production_paths.podway_home().map(|path| path.as_path())
    {
        return None;
    }
    let provisional = DevelopmentV2ProcessIdentityV1 {
        managed_root,
        account_root: metadata.account_root,
        dev_home,
        sandbox: metadata.sandbox,
        socket_path: active_paths.socket_path().as_path().to_path_buf(),
        state_directory: state_directory.to_path_buf(),
        daemon_path: daemon_path.clone(),
        daemon_sha256: metadata.snapshot.podwayd_sha256.clone(),
        production_paths,
        uid,
    };
    if !validate_process_directories_v1(&provisional, &metadata.snapshot.directory) {
        return None;
    }
    let bytes = read_owned_regular_bytes_v1(
        &daemon_path,
        uid,
        EXECUTABLE_FILE_MODE_V1,
        SERVICE_DAEMON_BINARY_MAX_BYTES_V1 as u64,
    )?;
    let daemon_sha256 = format!("{:x}", Sha256::digest(bytes));
    if daemon_sha256 != metadata.snapshot.podwayd_sha256 {
        return None;
    }
    Some(DevelopmentV2ProcessIdentityV1 {
        daemon_sha256,
        ..provisional
    })
}

#[cfg(all(feature = "development-v2-admission", debug_assertions))]
fn read_private_json_v1<T: serde::de::DeserializeOwned>(
    path: &Path,
    expected_uid: u32,
    maximum_bytes: u64,
) -> Option<T> {
    let bytes =
        read_owned_regular_bytes_v1(path, expected_uid, PRIVATE_FILE_MODE_V1, maximum_bytes)?;
    serde_json::from_slice(&bytes).ok()
}

#[cfg(all(feature = "development-v2-admission", debug_assertions))]
fn read_owned_regular_bytes_v1(
    path: &Path,
    expected_uid: u32,
    expected_mode: u32,
    maximum_bytes: u64,
) -> Option<Vec<u8>> {
    use std::os::unix::fs::MetadataExt as _;

    let descriptor = open(
        path,
        OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_RDONLY,
        Mode::empty(),
    )
    .ok()?;
    let mut file = File::from(descriptor);
    let metadata = file.metadata().ok()?;
    if !metadata.file_type().is_file()
        || metadata.uid() != expected_uid
        || metadata.mode() & 0o777 != expected_mode
        || metadata.len() == 0
        || metadata.len() > maximum_bytes
    {
        return None;
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.by_ref()
        .take(maximum_bytes + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    let after = file.metadata().ok()?;
    if bytes.len() as u64 != metadata.len()
        || bytes.len() as u64 > maximum_bytes
        || after.dev() != metadata.dev()
        || after.ino() != metadata.ino()
        || after.len() != metadata.len()
        || after.uid() != expected_uid
        || after.mode() & 0o777 != expected_mode
    {
        return None;
    }
    Some(bytes)
}

fn production_registry_excludes_v1(
    paths: &ServiceRuntimePathsV1,
    binding: &podway_store::WorkspaceBindingV1,
) -> bool {
    let Ok(registry) = load_registry_readonly_v1(paths) else {
        return false;
    };
    !registry.workspaces().iter().any(|entry| {
        entry.workspace_uuid() == binding.identity().workspace_uuid()
            || entry.last_known_root() == binding.last_validated_root()
    })
}

fn validate_private_directory_v1(path: &Path, expected_uid: u32) -> bool {
    use std::os::unix::fs::MetadataExt as _;

    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return false;
    };
    metadata.file_type().is_dir()
        && metadata.uid() == expected_uid
        && metadata.mode() & 0o777 == PRIVATE_DIRECTORY_MODE_V1
}

#[cfg(all(feature = "development-v2-admission", debug_assertions))]
fn validate_process_directories_v1(
    identity: &DevelopmentV2ProcessIdentityV1,
    snapshot_directory: &Path,
) -> bool {
    let snapshots = identity.managed_root.join("snapshots");
    let account_podway = identity.account_root.join(".podway");
    [
        identity.managed_root.as_path(),
        identity.account_root.as_path(),
        identity.dev_home.as_path(),
        identity.sandbox.as_path(),
        snapshots.as_path(),
        snapshot_directory,
        account_podway.as_path(),
    ]
    .iter()
    .all(|path| validate_private_directory_v1(path, identity.uid))
}

fn validate_request_directories_v1(identity: &DevelopmentV2ProcessIdentityV1) -> bool {
    let snapshots = identity.managed_root.join("snapshots");
    let account_podway = identity.account_root.join(".podway");
    let account_run = account_podway.join("run");
    let dev_run = identity.dev_home.join("run");
    let workspace_podway = identity.sandbox.join(".podway");
    let workspace_runtime = workspace_podway.join("runtime");
    let Some(snapshot_directory) = identity.daemon_path.parent() else {
        return false;
    };
    [
        identity.managed_root.as_path(),
        identity.account_root.as_path(),
        identity.dev_home.as_path(),
        identity.sandbox.as_path(),
        snapshots.as_path(),
        snapshot_directory,
        account_podway.as_path(),
        account_run.as_path(),
        dev_run.as_path(),
        identity.state_directory.as_path(),
        workspace_podway.as_path(),
        workspace_runtime.as_path(),
    ]
    .iter()
    .all(|path| validate_private_directory_v1(path, identity.uid))
}

#[cfg(all(feature = "development-v2-admission", debug_assertions))]
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct DevelopmentRuntimeMetadataV1 {
    schema: String,
    checkout: PathBuf,
    uid: u32,
    root: PathBuf,
    account_root: PathBuf,
    dev_home: PathBuf,
    sandbox: PathBuf,
    snapshot: DevelopmentRuntimeSnapshotV1,
}

#[cfg(all(feature = "development-v2-admission", debug_assertions))]
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct DevelopmentRuntimeSnapshotV1 {
    id: String,
    directory: PathBuf,
    podway: PathBuf,
    podwayd: PathBuf,
    podway_sha256: String,
    podwayd_sha256: String,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct DevelopmentV2MarkerV1 {
    schema: String,
    feature: String,
    uid: u32,
    managed_root: PathBuf,
    account_root: PathBuf,
    dev_home: PathBuf,
    workspace_root: PathBuf,
    socket_path: PathBuf,
    state_directory: PathBuf,
    daemon_path: PathBuf,
    daemon_sha256: String,
}

#[cfg(all(test, feature = "development-v2-admission", debug_assertions))]
mod tests {
    use std::{
        fs,
        os::unix::{ffi::OsStrExt as _, fs::PermissionsExt as _},
        path::{Path, PathBuf},
        process::Command,
        sync::{
            Arc,
            atomic::{AtomicU64, Ordering},
        },
    };

    use podway_core::{Sha256Digest, UnixMillis, WorkspaceId};
    use podway_git::{
        DiagnosticPathDisplayV1, GitResolverContractV1, LosslessPathV1, NativeGitResolverV1,
        ValidatedWorktreeV1, WORKTREE_SELECTOR_VERSION_V1, WorktreeSelectorV1,
    };
    use podway_protocol::{Rfc3339MillisV1, WorktreeSelectorWireV1};
    use podway_service::ServiceRuntimePathsV1;
    use podway_store::{
        DurableWorktreeIdentityV1, SqliteStoreOptionsV1, ValidatedWorkspaceRootV1,
        WorkspaceBindingV1,
    };
    use sha2::{Digest as _, Sha256};

    use super::{
        DEVELOPMENT_V2_ADMISSION_FEATURE_V1, DEVELOPMENT_V2_MARKER_SCHEMA_V1,
        DevelopmentV2AdmissionGateV1,
    };
    use crate::workspace::DEVELOPMENT_V2_MARKER_FILE_NAME_V1;
    use crate::{
        dispatch::WorkspaceRuntimeV1,
        production::{NativeProductionClockV1, ProductionWorkspaceRuntimeV1},
        runtime_workspace::{WorkspaceRuntimeManagerV1, WorkspaceRuntimeObservationV1},
    };

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct Fixture {
        checkout: PathBuf,
        root: PathBuf,
        account: PathBuf,
        dev_home: PathBuf,
        sandbox: PathBuf,
        daemon: PathBuf,
        digest: String,
        paths: ServiceRuntimePathsV1,
        binding: WorkspaceBindingV1,
        worktree: ValidatedWorktreeV1,
    }

    impl Fixture {
        fn new() -> Self {
            let uid = nix::unistd::geteuid().as_raw();
            let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
            let checkout = PathBuf::from(format!(
                "/private/tmp/podway-v2plt009-checkout-{}-{sequence}",
                std::process::id()
            ));
            fs::create_dir(&checkout).unwrap();
            fs::set_permissions(&checkout, fs::Permissions::from_mode(0o700)).unwrap();
            let checkout = checkout.canonicalize().unwrap();
            let checkout_digest = format!("{:x}", Sha256::digest(checkout.as_os_str().as_bytes()));
            let root = PathBuf::from(format!(
                "/private/tmp/podway-dev-{uid}-{}",
                &checkout_digest[..12]
            ));
            let account = root.join("account");
            let dev_home = root.join("dev");
            let sandbox = root.join("sandbox");
            let snapshot = root.join("snapshots/fixture");
            for directory in [
                &root,
                &account,
                &account.join(".podway"),
                &account.join(".podway/run"),
                &dev_home,
                &dev_home.join("run"),
                &dev_home.join("state"),
                &sandbox,
                &sandbox.join(".podway"),
                &sandbox.join(".podway/runtime"),
                &root.join("snapshots"),
                &snapshot,
            ] {
                fs::create_dir_all(directory).unwrap();
                fs::set_permissions(directory, fs::Permissions::from_mode(0o700)).unwrap();
            }
            let daemon = snapshot.join("podwayd");
            fs::write(&daemon, b"feature-enabled debug daemon fixture").unwrap();
            fs::set_permissions(&daemon, fs::Permissions::from_mode(0o755)).unwrap();
            let digest = format!("{:x}", Sha256::digest(fs::read(&daemon).unwrap()));
            let paths = ServiceRuntimePathsV1::for_dev_home(&account, &dev_home, uid).unwrap();
            run_git(&sandbox, &["init", "--quiet"]);
            run_git(
                &sandbox,
                &["config", "user.email", "podway@example.invalid"],
            );
            run_git(&sandbox, &["config", "user.name", "Podway Test"]);
            run_git(
                &sandbox,
                &["commit", "--quiet", "--allow-empty", "-m", "fixture"],
            );
            let worktree = NativeGitResolverV1::new()
                .resolve(selector(&sandbox))
                .unwrap();
            write_private_json(
                &root.join("runtime.json"),
                &serde_json::json!({
                    "schema": "podway.dev-runtime/v1",
                    "checkout": checkout,
                    "uid": uid,
                    "root": root,
                    "account_root": account,
                    "dev_home": dev_home,
                    "sandbox": sandbox,
                    "snapshot": {
                        "id": "fixture",
                        "directory": snapshot,
                        "podway": snapshot.join("podway"),
                        "podwayd": daemon,
                        "podway_sha256": "0".repeat(64),
                        "podwayd_sha256": digest,
                    }
                }),
            );
            let identity_digest = Sha256Digest::new(format!("sha256:{}", "a".repeat(64))).unwrap();
            let binding = WorkspaceBindingV1::new(
                DurableWorktreeIdentityV1::new(
                    identity_digest.clone(),
                    WorkspaceId::new("00000000-0000-4000-8000-000000000909").unwrap(),
                    identity_digest,
                ),
                ValidatedWorkspaceRootV1::from_path(&sandbox).unwrap(),
            );
            Self {
                checkout,
                root,
                account,
                dev_home,
                sandbox,
                daemon,
                digest,
                paths,
                binding,
                worktree,
            }
        }

        fn write_marker(&self) {
            write_private_json(
                &self
                    .sandbox
                    .join(".podway/runtime")
                    .join(DEVELOPMENT_V2_MARKER_FILE_NAME_V1),
                &serde_json::json!({
                    "schema": DEVELOPMENT_V2_MARKER_SCHEMA_V1,
                    "feature": DEVELOPMENT_V2_ADMISSION_FEATURE_V1,
                    "uid": nix::unistd::geteuid().as_raw(),
                    "managed_root": self.root,
                    "account_root": self.account,
                    "dev_home": self.dev_home,
                    "workspace_root": self.sandbox,
                    "socket_path": self.paths.socket_path().as_path(),
                    "state_directory": self.dev_home.join("state"),
                    "daemon_path": self.daemon,
                    "daemon_sha256": self.digest,
                }),
            );
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
            let _ = fs::remove_dir_all(&self.checkout);
        }
    }

    fn write_private_json(path: &Path, value: &serde_json::Value) {
        fs::write(path, podway_core::canonicalize_json_v1(value).unwrap()).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }

    fn run_git(root: &Path, arguments: &[&str]) {
        let status = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(arguments)
            .status()
            .unwrap();
        assert!(status.success());
    }

    fn selector(root: &Path) -> WorktreeSelectorV1 {
        let root = fs::canonicalize(root).unwrap();
        let display = DiagnosticPathDisplayV1::new("development-v2 fixture").unwrap();
        let path = LosslessPathV1::from_raw_bytes(root.as_os_str().as_bytes(), display).unwrap();
        WorktreeSelectorV1::new(WORKTREE_SELECTOR_VERSION_V1, None, path).unwrap()
    }

    fn observation() -> WorkspaceRuntimeObservationV1 {
        WorkspaceRuntimeObservationV1::new(
            UnixMillis::new(1_700_000_000_123),
            Rfc3339MillisV1::new("2026-08-09T00:00:00.123Z").unwrap(),
        )
    }

    #[test]
    fn v2plt009_gate_requires_dev_process_and_exact_disposable_marker() {
        let fixture = Fixture::new();
        let production =
            DevelopmentV2AdmissionGateV1::from_process(false, &fixture.paths, &fixture.daemon);
        assert!(!production.process_is_eligible());

        let gate =
            DevelopmentV2AdmissionGateV1::from_process(true, &fixture.paths, &fixture.daemon);
        assert!(gate.process_is_eligible());
        assert!(!gate.permits_workspace(&fixture.binding, &fixture.worktree));

        fixture.write_marker();
        assert!(gate.permits_workspace(&fixture.binding, &fixture.worktree));

        let marker = fixture
            .sandbox
            .join(".podway/runtime")
            .join(DEVELOPMENT_V2_MARKER_FILE_NAME_V1);
        let marker_value: serde_json::Value =
            serde_json::from_slice(&fs::read(&marker).unwrap()).unwrap();
        fs::write(&marker, serde_json::to_vec_pretty(&marker_value).unwrap()).unwrap();
        assert!(!gate.permits_workspace(&fixture.binding, &fixture.worktree));

        fixture.write_marker();
        fs::set_permissions(marker, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(!gate.permits_workspace(&fixture.binding, &fixture.worktree));

        fixture.write_marker();
        fs::set_permissions(&fixture.sandbox, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(!gate.permits_workspace(&fixture.binding, &fixture.worktree));
    }

    #[test]
    fn v2plt009_gate_rejects_marker_tamper_and_normal_registry_membership() {
        let fixture = Fixture::new();
        fixture.write_marker();
        let mut gate =
            DevelopmentV2AdmissionGateV1::from_process(true, &fixture.paths, &fixture.daemon);
        assert!(gate.permits_workspace(&fixture.binding, &fixture.worktree));

        let marker = fixture
            .sandbox
            .join(".podway/runtime")
            .join(DEVELOPMENT_V2_MARKER_FILE_NAME_V1);
        let mut value: serde_json::Value =
            serde_json::from_slice(&fs::read(&marker).unwrap()).unwrap();
        value["daemon_sha256"] = serde_json::Value::String("0".repeat(64));
        write_private_json(&marker, &value);
        assert!(!gate.permits_workspace(&fixture.binding, &fixture.worktree));

        fixture.write_marker();
        let production_account = fixture.root.join("production-account");
        let production_home = fixture.root.join("production-home");
        for directory in [
            &production_account,
            &production_home,
            &production_home.join("state"),
        ] {
            fs::create_dir_all(directory).unwrap();
            fs::set_permissions(directory, fs::Permissions::from_mode(0o700)).unwrap();
        }
        let production_paths = ServiceRuntimePathsV1::for_dev_home(
            &production_account,
            &production_home,
            nix::unistd::geteuid().as_raw(),
        )
        .unwrap();
        let registry_path = production_paths
            .workspace_registry_path()
            .as_path()
            .to_path_buf();
        gate.enabled.as_mut().unwrap().production_paths = production_paths;
        write_private_json(
            &registry_path,
            &serde_json::json!({
                "schema": "podway.registry/v1",
                "workspaces": [{
                    "workspace_uuid": fixture.binding.identity().workspace_uuid().as_str(),
                    "last_known_root": fixture.binding.last_validated_root().as_encoded(),
                    "last_seen_at": "2026-08-09T00:00:00.000Z"
                }]
            }),
        );
        assert!(!gate.permits_workspace(&fixture.binding, &fixture.worktree));
    }

    #[test]
    fn v2plt009_gate_rejects_installed_or_nonisolated_endpoint_topology() {
        let fixture = Fixture::new();
        fixture.write_marker();

        let installed_copy = fixture.root.join("installed/podwayd");
        fs::create_dir_all(installed_copy.parent().unwrap()).unwrap();
        fs::copy(&fixture.daemon, &installed_copy).unwrap();
        fs::set_permissions(&installed_copy, fs::Permissions::from_mode(0o755)).unwrap();
        let installed =
            DevelopmentV2AdmissionGateV1::from_process(true, &fixture.paths, &installed_copy);
        assert!(!installed.process_is_eligible());

        let alternate_socket = fixture.dev_home.join("run/not-the-managed-endpoint.sock");
        let wrong_endpoint = fixture
            .paths
            .clone()
            .with_socket_path(&alternate_socket)
            .unwrap();
        let wrong_endpoint =
            DevelopmentV2AdmissionGateV1::from_process(true, &wrong_endpoint, &fixture.daemon);
        assert!(!wrong_endpoint.process_is_eligible());

        let original_metadata: serde_json::Value =
            serde_json::from_slice(&fs::read(fixture.root.join("runtime.json")).unwrap()).unwrap();
        let mut metadata = original_metadata.clone();
        metadata["checkout"] =
            serde_json::Value::String(fixture.sandbox.to_string_lossy().into_owned());
        write_private_json(&fixture.root.join("runtime.json"), &metadata);
        let wrong_root =
            DevelopmentV2AdmissionGateV1::from_process(true, &fixture.paths, &fixture.daemon);
        assert!(!wrong_root.process_is_eligible());

        let mut metadata = original_metadata;
        metadata["snapshot"]["podwayd_sha256"] = serde_json::Value::String("0".repeat(64));
        write_private_json(&fixture.root.join("runtime.json"), &metadata);
        let wrong_digest =
            DevelopmentV2AdmissionGateV1::from_process(true, &fixture.paths, &fixture.daemon);
        assert!(!wrong_digest.process_is_eligible());
    }

    #[test]
    fn v2plt009_production_runtime_grants_only_after_readonly_resolution() {
        let fixture = Fixture::new();
        let bootstrap_manager =
            WorkspaceRuntimeManagerV1::new(&fixture.paths, SqliteStoreOptionsV1::new(8).unwrap());
        bootstrap_manager
            .bootstrap(selector(&fixture.sandbox), observation())
            .unwrap();
        drop(bootstrap_manager);
        let manager = Arc::new(WorkspaceRuntimeManagerV1::new(
            &fixture.paths,
            SqliteStoreOptionsV1::new(8).unwrap(),
        ));
        fixture.write_marker();

        let mut gate =
            DevelopmentV2AdmissionGateV1::from_process(true, &fixture.paths, &fixture.daemon);
        let production_account = fixture.root.join("isolated-production-account");
        let production_home = fixture.root.join("isolated-production-home");
        fs::create_dir_all(&production_account).unwrap();
        fs::create_dir_all(&production_home).unwrap();
        fs::set_permissions(&production_account, fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(&production_home, fs::Permissions::from_mode(0o700)).unwrap();
        gate.enabled.as_mut().unwrap().production_paths = ServiceRuntimePathsV1::for_dev_home(
            &production_account,
            &production_home,
            nix::unistd::geteuid().as_raw(),
        )
        .unwrap();
        let production_state = production_home.join("state");
        assert!(!production_state.exists());

        let runtime = ProductionWorkspaceRuntimeV1::new(
            Arc::clone(&manager),
            Arc::new(NativeProductionClockV1::default()),
        )
        .with_development_v2_admission(gate);
        let selector = WorktreeSelectorWireV1::new(
            fs::canonicalize(&fixture.sandbox)
                .unwrap()
                .as_os_str()
                .as_bytes(),
            "development-v2 fixture",
            None,
        )
        .unwrap();
        let registry_path = fixture.paths.workspace_registry_path().as_path();
        let database_path = fixture.sandbox.join(".podway/runtime/state.sqlite3");
        let registry_before = fs::read(registry_path).unwrap();
        let database_before = fs::read(&database_path).unwrap();

        assert!(runtime.development_v2_admission(&selector).is_some());
        assert!(!production_state.exists());
        assert_eq!(fs::read(registry_path).unwrap(), registry_before);
        assert_eq!(fs::read(&database_path).unwrap(), database_before);

        fs::remove_file(
            fixture
                .sandbox
                .join(".podway/runtime")
                .join(DEVELOPMENT_V2_MARKER_FILE_NAME_V1),
        )
        .unwrap();
        assert!(runtime.development_v2_admission(&selector).is_none());
        assert!(!production_state.exists());
        assert_eq!(fs::read(registry_path).unwrap(), registry_before);
        assert_eq!(fs::read(&database_path).unwrap(), database_before);

        std::os::unix::fs::symlink(
            fixture.root.join("runtime.json"),
            fixture
                .sandbox
                .join(".podway/runtime")
                .join(DEVELOPMENT_V2_MARKER_FILE_NAME_V1),
        )
        .unwrap();
        assert!(runtime.development_v2_admission(&selector).is_none());
        assert!(!production_state.exists());
        assert_eq!(fs::read(registry_path).unwrap(), registry_before);
        assert_eq!(fs::read(&database_path).unwrap(), database_before);
    }
}
