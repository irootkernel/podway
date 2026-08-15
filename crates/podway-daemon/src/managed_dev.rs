//! Validated process topology for managed `podwayd --dev` runtimes.

use std::{
    error::Error,
    fmt, fs,
    os::unix::fs::{MetadataExt as _, PermissionsExt as _},
    path::{Path, PathBuf},
};

use sha2::{Digest as _, Sha256};

pub const MANAGED_DEV_RUNTIME_SCHEMA_V2: &str = "podway.managed-dev-runtime/v2";
const MAXIMUM_METADATA_BYTES: u64 = 16 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ManagedDevPurposeV2 {
    Contributor,
    ReleaseQualification,
}

#[derive(Clone, Debug)]
pub struct ManagedDevRuntimeV2 {
    purpose: ManagedDevPurposeV2,
    account_root: PathBuf,
    dev_home: PathBuf,
    sandbox: PathBuf,
}

impl ManagedDevRuntimeV2 {
    /// Loads metadata adjacent to `dev_home`. Absence preserves legacy raw `--dev` behavior;
    /// presence is authoritative and therefore any validation failure is fatal.
    pub fn discover(
        dev_home: &Path,
        current_executable: &Path,
    ) -> Result<Option<Self>, ManagedDevRuntimeErrorV2> {
        let Some(root) = dev_home.parent() else {
            return Ok(None);
        };
        let metadata_path = root.join("runtime.json");
        match fs::symlink_metadata(&metadata_path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(source) => return Err(ManagedDevRuntimeErrorV2::Io(source)),
            Ok(metadata) => validate_file(&metadata, 0o600)?,
        }
        let metadata = fs::metadata(&metadata_path).map_err(ManagedDevRuntimeErrorV2::Io)?;
        if metadata.len() > MAXIMUM_METADATA_BYTES {
            return Err(ManagedDevRuntimeErrorV2::Invalid("metadata is too large"));
        }
        let bytes = fs::read(&metadata_path).map_err(ManagedDevRuntimeErrorV2::Io)?;
        let document: ManagedDevRuntimeDocumentV2 =
            serde_json::from_slice(&bytes).map_err(ManagedDevRuntimeErrorV2::Json)?;
        let _ = &document.checkout;
        let uid = nix::unistd::geteuid().as_raw();
        let root = root.canonicalize().map_err(ManagedDevRuntimeErrorV2::Io)?;
        let expected_prefix = match document.purpose {
            ManagedDevPurposeV2::Contributor => format!("podway-dev-{uid}-"),
            ManagedDevPurposeV2::ReleaseQualification => format!("podway-release-{uid}-"),
        };
        if document.schema != MANAGED_DEV_RUNTIME_SCHEMA_V2
            || document.uid != uid
            || root.parent() != Some(Path::new("/private/tmp"))
            || !root
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with(&expected_prefix))
            || document.root != root
            || document.account_root != root.join("account")
            || document.dev_home != root.join("dev")
            || document.sandbox != root.join("sandbox")
            || document.snapshot.directory != root.join("snapshots").join(&document.snapshot.id)
            || document.snapshot.podway != document.snapshot.directory.join("podway")
            || document.snapshot.podwayd != document.snapshot.directory.join("podwayd")
        {
            return Err(ManagedDevRuntimeErrorV2::Invalid(
                "metadata topology does not match its managed root",
            ));
        }
        for path in [
            &document.root,
            &document.account_root,
            &document.dev_home,
            &document.sandbox,
            &document.snapshot.directory,
        ] {
            let metadata = fs::symlink_metadata(path).map_err(ManagedDevRuntimeErrorV2::Io)?;
            validate_directory(&metadata, 0o700)?;
        }
        validate_snapshot(
            &document.snapshot.podway,
            &document.snapshot.podway_sha256,
            uid,
        )?;
        validate_snapshot(
            &document.snapshot.podwayd,
            &document.snapshot.podwayd_sha256,
            uid,
        )?;
        let executable = current_executable
            .canonicalize()
            .map_err(ManagedDevRuntimeErrorV2::Io)?;
        let daemon = document
            .snapshot
            .podwayd
            .canonicalize()
            .map_err(ManagedDevRuntimeErrorV2::Io)?;
        if executable != daemon {
            return Err(ManagedDevRuntimeErrorV2::Invalid(
                "current daemon is not the declared snapshot",
            ));
        }
        Ok(Some(Self {
            purpose: document.purpose,
            account_root: document.account_root,
            dev_home: document.dev_home,
            sandbox: document.sandbox,
        }))
    }

    pub const fn purpose(&self) -> ManagedDevPurposeV2 {
        self.purpose
    }

    pub fn account_root(&self) -> &Path {
        &self.account_root
    }

    pub fn dev_home(&self) -> &Path {
        &self.dev_home
    }

    pub fn sandbox(&self) -> &Path {
        &self.sandbox
    }
}

#[derive(Debug)]
pub enum ManagedDevRuntimeErrorV2 {
    Io(std::io::Error),
    Json(serde_json::Error),
    Invalid(&'static str),
}

impl fmt::Display for ManagedDevRuntimeErrorV2 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(_) => formatter.write_str("cannot inspect managed dev runtime"),
            Self::Json(_) => formatter.write_str("managed dev runtime metadata is invalid"),
            Self::Invalid(message) => {
                write!(formatter, "managed dev runtime is invalid: {message}")
            }
        }
    }
}

impl Error for ManagedDevRuntimeErrorV2 {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(source) => Some(source),
            Self::Json(source) => Some(source),
            Self::Invalid(_) => None,
        }
    }
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ManagedDevRuntimeDocumentV2 {
    schema: String,
    purpose: ManagedDevPurposeV2,
    #[serde(default)]
    checkout: Option<PathBuf>,
    uid: u32,
    root: PathBuf,
    account_root: PathBuf,
    dev_home: PathBuf,
    sandbox: PathBuf,
    snapshot: ManagedDevSnapshotV2,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ManagedDevSnapshotV2 {
    id: String,
    directory: PathBuf,
    podway: PathBuf,
    podwayd: PathBuf,
    podway_sha256: String,
    podwayd_sha256: String,
}

fn validate_directory(metadata: &fs::Metadata, mode: u32) -> Result<(), ManagedDevRuntimeErrorV2> {
    if metadata.file_type().is_symlink()
        || !metadata.is_dir()
        || metadata.uid() != nix::unistd::geteuid().as_raw()
        || metadata.permissions().mode() & 0o777 != mode
    {
        return Err(ManagedDevRuntimeErrorV2::Invalid(
            "managed directory ownership or mode is unsafe",
        ));
    }
    Ok(())
}

fn validate_file(metadata: &fs::Metadata, mode: u32) -> Result<(), ManagedDevRuntimeErrorV2> {
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.uid() != nix::unistd::geteuid().as_raw()
        || metadata.permissions().mode() & 0o777 != mode
    {
        return Err(ManagedDevRuntimeErrorV2::Invalid(
            "managed file ownership or mode is unsafe",
        ));
    }
    Ok(())
}

fn validate_snapshot(
    path: &Path,
    expected: &str,
    uid: u32,
) -> Result<(), ManagedDevRuntimeErrorV2> {
    let metadata = fs::symlink_metadata(path).map_err(ManagedDevRuntimeErrorV2::Io)?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.uid() != uid
        || metadata.permissions().mode() & 0o777 != 0o755
        || expected.len() != 64
        || !expected
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(ManagedDevRuntimeErrorV2::Invalid(
            "managed executable ownership, mode, or digest is invalid",
        ));
    }
    let actual = format!(
        "{:x}",
        Sha256::digest(fs::read(path).map_err(ManagedDevRuntimeErrorV2::Io)?)
    );
    if actual != expected {
        return Err(ManagedDevRuntimeErrorV2::Invalid(
            "managed executable digest does not match metadata",
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        os::unix::fs::PermissionsExt as _,
        path::{Path, PathBuf},
        sync::atomic::{AtomicU64, Ordering},
    };

    use sha2::{Digest as _, Sha256};

    use super::{ManagedDevPurposeV2, ManagedDevRuntimeV2};

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct Fixture {
        root: PathBuf,
        daemon: PathBuf,
    }

    impl Fixture {
        fn new() -> Self {
            let uid = nix::unistd::geteuid().as_raw();
            let root = PathBuf::from(format!(
                "/private/tmp/podway-release-{uid}-test-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            let snapshot = root.join("snapshots/fixture");
            for directory in [
                root.clone(),
                root.join("account"),
                root.join("dev"),
                root.join("sandbox"),
                root.join("snapshots"),
                snapshot.clone(),
            ] {
                fs::create_dir(&directory).expect("managed fixture directory");
                fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
                    .expect("managed fixture directory mode");
            }
            let cli = snapshot.join("podway");
            let daemon = snapshot.join("podwayd");
            write_executable(&cli, b"cli");
            write_executable(&daemon, b"daemon");
            let document = serde_json::json!({
                "schema": "podway.managed-dev-runtime/v2",
                "purpose": "release-qualification",
                "uid": uid,
                "root": root,
                "account_root": root.join("account"),
                "dev_home": root.join("dev"),
                "sandbox": root.join("sandbox"),
                "snapshot": {
                    "id": "fixture",
                    "directory": snapshot,
                    "podway": cli,
                    "podwayd": daemon,
                    "podway_sha256": format!("{:x}", Sha256::digest(b"cli")),
                    "podwayd_sha256": format!("{:x}", Sha256::digest(b"daemon")),
                }
            });
            let metadata = root.join("runtime.json");
            fs::write(
                &metadata,
                serde_json::to_vec(&document).expect("metadata JSON"),
            )
            .expect("managed fixture metadata");
            fs::set_permissions(&metadata, fs::Permissions::from_mode(0o600))
                .expect("managed fixture metadata mode");
            Self { root, daemon }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn write_executable(path: &Path, bytes: &[u8]) {
        fs::write(path, bytes).expect("managed fixture executable");
        fs::set_permissions(path, fs::Permissions::from_mode(0o755))
            .expect("managed fixture executable mode");
    }

    #[test]
    fn absent_metadata_preserves_raw_dev_discovery() {
        let root = PathBuf::from(format!(
            "/private/tmp/podway-raw-dev-test-{}",
            std::process::id()
        ));
        assert!(
            ManagedDevRuntimeV2::discover(&root.join("dev"), Path::new("/bin/false"))
                .expect("missing metadata is not an error")
                .is_none()
        );
    }

    #[test]
    fn valid_release_metadata_is_isolated_and_tampering_fails_closed() {
        let fixture = Fixture::new();
        let runtime = ManagedDevRuntimeV2::discover(&fixture.root.join("dev"), &fixture.daemon)
            .expect("valid metadata")
            .expect("managed runtime");
        assert_eq!(runtime.purpose(), ManagedDevPurposeV2::ReleaseQualification);
        assert_eq!(runtime.account_root(), fixture.root.join("account"));

        let metadata = fixture.root.join("runtime.json");
        let mut document: serde_json::Value =
            serde_json::from_slice(&fs::read(&metadata).expect("metadata bytes"))
                .expect("metadata JSON");
        document["account_root"] = serde_json::Value::String("/private/tmp/escape".to_owned());
        fs::write(
            &metadata,
            serde_json::to_vec(&document).expect("tampered JSON"),
        )
        .expect("tampered metadata");
        assert!(ManagedDevRuntimeV2::discover(&fixture.root.join("dev"), &fixture.daemon).is_err());
    }
}
