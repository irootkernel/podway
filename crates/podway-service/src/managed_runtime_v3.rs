//! Closed validation for optional `podway.managed-runtime/v3` metadata.

use std::{
    error::Error,
    fmt, fs,
    os::unix::fs::{MetadataExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
};

use nix::unistd::geteuid;
use podway_core::RuntimeModeV1;
use serde::Deserialize;
use sha2::{Digest as _, Sha256};

use crate::{PodwayHomeV1, ServiceRuntimePathsV1};

pub const MANAGED_RUNTIME_SCHEMA_V3: &str = "podway.managed-runtime/v3";
pub const MANAGED_RUNTIME_METADATA_VERSION_V3: u8 = 3;
pub const MANAGED_RUNTIME_METADATA_NAME_V3: &str = "runtime.json";
const MAXIMUM_MANAGED_RUNTIME_METADATA_BYTES_V3: u64 = 16 * 1024;

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "kebab-case")]
pub enum ManagedRuntimePurposeV3 {
    AquariumDevelopment,
    Contributor,
    ReleaseQualification,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ManagedRuntimeExecutableRoleV3 {
    Cli,
    Daemon,
    Controller,
}

#[derive(Clone, Debug)]
pub struct ManagedRuntimeV3 {
    purpose: ManagedRuntimePurposeV3,
    canonical_root: PathBuf,
    mode: RuntimeModeV1,
    paths: ServiceRuntimePathsV1,
    sandbox_root: Option<PathBuf>,
    generation: Option<String>,
}

impl ManagedRuntimeV3 {
    /// Discovers authoritative metadata at `<runtime-root>/runtime.json`.
    ///
    /// Absence denotes an unmanaged runtime. Presence is fail-closed: every
    /// identity, topology, ownership, permission, and executable mismatch is an
    /// error and never falls back to another mode or endpoint.
    pub fn discover(
        runtime_root: &Path,
        selected_mode: &RuntimeModeV1,
        current_executable: &Path,
        role: ManagedRuntimeExecutableRoleV3,
    ) -> Result<Option<Self>, ManagedRuntimeErrorV3> {
        let metadata_path = runtime_root.join(MANAGED_RUNTIME_METADATA_NAME_V3);
        let metadata = match fs::symlink_metadata(&metadata_path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => return Err(ManagedRuntimeErrorV3::Io(source)),
            Ok(metadata) => metadata,
        };
        validate_file_metadata(&metadata, 0o600, false)?;
        if metadata.len() > MAXIMUM_MANAGED_RUNTIME_METADATA_BYTES_V3 {
            return Err(ManagedRuntimeErrorV3::Invalid("metadata is too large"));
        }
        let bytes = fs::read(&metadata_path).map_err(ManagedRuntimeErrorV3::Io)?;
        let document: ManagedRuntimeDocumentV3 =
            serde_json::from_slice(&bytes).map_err(ManagedRuntimeErrorV3::Json)?;
        let mode = RuntimeModeV1::new(document.mode.clone())
            .map_err(|_| ManagedRuntimeErrorV3::Invalid("mode is invalid"))?;
        let canonical_root = runtime_root
            .canonicalize()
            .map_err(ManagedRuntimeErrorV3::Io)?;
        if runtime_root != canonical_root || document.canonical_root != canonical_root {
            return Err(ManagedRuntimeErrorV3::Invalid(
                "canonical root does not match the selected runtime root",
            ));
        }
        if document.schema != MANAGED_RUNTIME_SCHEMA_V3
            || document.metadata_version != MANAGED_RUNTIME_METADATA_VERSION_V3
            || document.euid != geteuid().as_raw()
            || &mode != selected_mode
        {
            return Err(ManagedRuntimeErrorV3::Invalid(
                "metadata identity does not match the selected runtime",
            ));
        }
        validate_directory(&canonical_root)?;
        let production_root = PodwayHomeV1::for_effective_user().map_err(|_| {
            ManagedRuntimeErrorV3::Invalid("production runtime root is unavailable")
        })?;
        if canonical_root == production_root.as_path() {
            return Err(ManagedRuntimeErrorV3::Invalid(
                "managed runtime root aliases the production runtime",
            ));
        }
        let paths =
            ServiceRuntimePathsV1::for_runtime_root(&canonical_root, mode.clone(), document.euid)
                .map_err(|_| ManagedRuntimeErrorV3::Invalid("runtime paths are invalid"))?;
        if document.paths.lock != paths.global_lock_path().as_path()
            || document.paths.socket != paths.socket_path().as_path()
            || document.paths.service_state != paths.metadata_index_path().as_path()
            || document.paths.registry != paths.workspace_registry_path().as_path()
            || document.paths.recovery != paths.recovery_path().as_path()
            || document.paths.log != paths.log_path().as_path()
            || document.paths.bootstrap_log != paths.bootstrap_log_path().as_path()
        {
            return Err(ManagedRuntimeErrorV3::Invalid(
                "metadata paths do not match the selected runtime namespace",
            ));
        }
        validate_purpose_shape(&document, &mode)?;
        if let Some(sandbox) = document.sandbox_root.as_deref() {
            let canonical = sandbox.canonicalize().map_err(ManagedRuntimeErrorV3::Io)?;
            if canonical != sandbox {
                return Err(ManagedRuntimeErrorV3::Invalid(
                    "sandbox root is not canonical",
                ));
            }
            validate_directory(sandbox)?;
        }
        validate_executable(&document.executables.cli)?;
        validate_executable(&document.executables.daemon)?;
        if let Some(controller) = document.executables.controller.as_ref() {
            validate_executable(controller)?;
        }
        let declared = match role {
            ManagedRuntimeExecutableRoleV3::Cli => Some(&document.executables.cli),
            ManagedRuntimeExecutableRoleV3::Daemon => Some(&document.executables.daemon),
            ManagedRuntimeExecutableRoleV3::Controller => document.executables.controller.as_ref(),
        }
        .ok_or(ManagedRuntimeErrorV3::Invalid(
            "the selected executable role is not declared",
        ))?;
        let current = current_executable
            .canonicalize()
            .map_err(ManagedRuntimeErrorV3::Io)?;
        if current != declared.path {
            return Err(ManagedRuntimeErrorV3::Invalid(
                "current executable is not the declared managed executable",
            ));
        }
        Ok(Some(Self {
            purpose: document.purpose,
            canonical_root,
            mode,
            paths,
            sandbox_root: document.sandbox_root,
            generation: document.generation,
        }))
    }

    pub const fn purpose(&self) -> ManagedRuntimePurposeV3 {
        self.purpose
    }

    pub fn canonical_root(&self) -> &Path {
        &self.canonical_root
    }

    pub fn mode(&self) -> &RuntimeModeV1 {
        &self.mode
    }

    pub fn paths(&self) -> &ServiceRuntimePathsV1 {
        &self.paths
    }

    pub fn sandbox_root(&self) -> Option<&Path> {
        self.sandbox_root.as_deref()
    }

    pub fn generation(&self) -> Option<&str> {
        self.generation.as_deref()
    }
}

#[derive(Debug)]
pub enum ManagedRuntimeErrorV3 {
    Io(std::io::Error),
    Json(serde_json::Error),
    Invalid(&'static str),
}

impl fmt::Display for ManagedRuntimeErrorV3 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(_) => formatter.write_str("cannot inspect managed runtime"),
            Self::Json(_) => formatter.write_str("managed runtime metadata is invalid"),
            Self::Invalid(reason) => write!(formatter, "managed runtime is invalid: {reason}"),
        }
    }
}

impl Error for ManagedRuntimeErrorV3 {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(source) => Some(source),
            Self::Json(source) => Some(source),
            Self::Invalid(_) => None,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ManagedRuntimeDocumentV3 {
    schema: String,
    metadata_version: u8,
    purpose: ManagedRuntimePurposeV3,
    euid: u32,
    canonical_root: PathBuf,
    mode: String,
    paths: ManagedRuntimePathsDocumentV3,
    sandbox_root: Option<PathBuf>,
    executables: ManagedRuntimeExecutablesDocumentV3,
    generation: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ManagedRuntimePathsDocumentV3 {
    lock: PathBuf,
    socket: PathBuf,
    service_state: PathBuf,
    registry: PathBuf,
    recovery: PathBuf,
    log: PathBuf,
    bootstrap_log: PathBuf,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ManagedRuntimeExecutablesDocumentV3 {
    cli: ManagedRuntimeExecutableDocumentV3,
    daemon: ManagedRuntimeExecutableDocumentV3,
    controller: Option<ManagedRuntimeExecutableDocumentV3>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ManagedRuntimeExecutableDocumentV3 {
    path: PathBuf,
    sha256: String,
}

fn validate_purpose_shape(
    document: &ManagedRuntimeDocumentV3,
    mode: &RuntimeModeV1,
) -> Result<(), ManagedRuntimeErrorV3> {
    match document.purpose {
        ManagedRuntimePurposeV3::AquariumDevelopment => {
            if mode.as_str() != "dev"
                || document.sandbox_root.is_some()
                || document.executables.controller.is_none()
                || document.generation.as_deref().is_none_or(str::is_empty)
            {
                return Err(ManagedRuntimeErrorV3::Invalid(
                    "Aquarium development metadata has an invalid purpose shape",
                ));
            }
        }
        ManagedRuntimePurposeV3::Contributor | ManagedRuntimePurposeV3::ReleaseQualification => {
            if document.sandbox_root.is_none()
                || document.executables.controller.is_some()
                || document.generation.is_some()
            {
                return Err(ManagedRuntimeErrorV3::Invalid(
                    "private runtime metadata has an invalid purpose shape",
                ));
            }
        }
    }
    Ok(())
}

fn validate_directory(path: &Path) -> Result<(), ManagedRuntimeErrorV3> {
    let metadata = fs::symlink_metadata(path).map_err(ManagedRuntimeErrorV3::Io)?;
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || metadata.uid() != geteuid().as_raw()
        || metadata.permissions().mode() & 0o777 != 0o700
    {
        return Err(ManagedRuntimeErrorV3::Invalid(
            "managed directory ownership or mode is unsafe",
        ));
    }
    Ok(())
}

fn validate_file_metadata(
    metadata: &fs::Metadata,
    expected_mode: u32,
    executable: bool,
) -> Result<(), ManagedRuntimeErrorV3> {
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.uid() != geteuid().as_raw()
        || metadata.permissions().mode() & 0o777 != expected_mode
        || executable && expected_mode & 0o111 == 0
    {
        return Err(ManagedRuntimeErrorV3::Invalid(
            "managed file ownership or mode is unsafe",
        ));
    }
    Ok(())
}

fn validate_executable(
    executable: &ManagedRuntimeExecutableDocumentV3,
) -> Result<(), ManagedRuntimeErrorV3> {
    if !executable.path.is_absolute()
        || executable.sha256.len() != 71
        || !executable.sha256.starts_with("sha256:")
        || !executable.sha256[7..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        return Err(ManagedRuntimeErrorV3::Invalid(
            "managed executable identity is invalid",
        ));
    }
    let metadata = fs::symlink_metadata(&executable.path).map_err(ManagedRuntimeErrorV3::Io)?;
    validate_file_metadata(&metadata, 0o755, true)?;
    let canonical = executable
        .path
        .canonicalize()
        .map_err(ManagedRuntimeErrorV3::Io)?;
    if canonical != executable.path {
        return Err(ManagedRuntimeErrorV3::Invalid(
            "managed executable path is not canonical",
        ));
    }
    let bytes = fs::read(&executable.path).map_err(ManagedRuntimeErrorV3::Io)?;
    let actual = format!("sha256:{:x}", Sha256::digest(bytes));
    if actual != executable.sha256 {
        return Err(ManagedRuntimeErrorV3::Invalid(
            "managed executable digest does not match",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        os::unix::fs::PermissionsExt as _,
        time::{SystemTime, UNIX_EPOCH},
    };

    use nix::unistd::geteuid;
    use serde_json::json;
    use sha2::{Digest as _, Sha256};

    use super::{ManagedRuntimeExecutableRoleV3, ManagedRuntimeV3};
    use crate::ServiceRuntimePathsV1;
    use podway_core::RuntimeModeV1;

    struct Fixture {
        root: std::path::PathBuf,
        cli: std::path::PathBuf,
    }

    impl Fixture {
        fn contributor() -> Self {
            let suffix = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let root = std::path::PathBuf::from(format!(
                "/private/tmp/pw3-{}-{suffix}",
                std::process::id()
            ));
            fs::create_dir(&root).unwrap();
            fs::set_permissions(&root, fs::Permissions::from_mode(0o700)).unwrap();
            let sandbox = root.join("sandbox");
            fs::create_dir(&sandbox).unwrap();
            fs::set_permissions(&sandbox, fs::Permissions::from_mode(0o700)).unwrap();
            let cli = root.join("podway");
            let daemon = root.join("podwayd");
            for (path, bytes) in [(&cli, b"cli".as_slice()), (&daemon, b"daemon".as_slice())] {
                fs::write(path, bytes).unwrap();
                fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
            }
            let mode = RuntimeModeV1::new("contributor").unwrap();
            let paths =
                ServiceRuntimePathsV1::for_runtime_root(&root, mode, geteuid().as_raw()).unwrap();
            let digest = |path: &std::path::Path| {
                format!("sha256:{:x}", Sha256::digest(fs::read(path).unwrap()))
            };
            let document = json!({
                "schema": "podway.managed-runtime/v3",
                "metadata_version": 3,
                "purpose": "contributor",
                "euid": geteuid().as_raw(),
                "canonical_root": root,
                "mode": "contributor",
                "paths": {
                    "lock": paths.global_lock_path().as_path(),
                    "socket": paths.socket_path().as_path(),
                    "service_state": paths.metadata_index_path().as_path(),
                    "registry": paths.workspace_registry_path().as_path(),
                    "recovery": paths.recovery_path().as_path(),
                    "log": paths.log_path().as_path(),
                    "bootstrap_log": paths.bootstrap_log_path().as_path(),
                },
                "sandbox_root": sandbox,
                "executables": {
                    "cli": {"path": cli, "sha256": digest(&cli)},
                    "daemon": {"path": daemon, "sha256": digest(&daemon)},
                    "controller": null,
                },
                "generation": null,
            });
            let metadata = root.join("runtime.json");
            fs::write(&metadata, serde_json::to_vec(&document).unwrap()).unwrap();
            fs::set_permissions(&metadata, fs::Permissions::from_mode(0o600)).unwrap();
            Self { root, cli }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn validates_exact_managed_runtime_v3_identity() {
        let fixture = Fixture::contributor();
        let runtime = ManagedRuntimeV3::discover(
            &fixture.root,
            &RuntimeModeV1::new("contributor").unwrap(),
            &fixture.cli,
            ManagedRuntimeExecutableRoleV3::Cli,
        )
        .unwrap()
        .unwrap();
        assert_eq!(runtime.mode().as_str(), "contributor");
        assert_eq!(
            runtime.sandbox_root(),
            Some(fixture.root.join("sandbox").as_path())
        );
    }

    #[test]
    fn rejects_selected_mode_mismatch_without_fallback() {
        let fixture = Fixture::contributor();
        assert!(
            ManagedRuntimeV3::discover(
                &fixture.root,
                &RuntimeModeV1::new("release-qa").unwrap(),
                &fixture.cli,
                ManagedRuntimeExecutableRoleV3::Cli,
            )
            .is_err()
        );
    }
}
