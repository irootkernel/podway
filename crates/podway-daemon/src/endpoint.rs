//! Per-user Unix-domain endpoint ownership for the daemon.
//!
//! Paths are supplied exclusively by `podway-service`. This module only validates the filesystem
//! objects at those paths before claiming the singleton lock and socket.

use std::{
    error::Error,
    fmt,
    fs::{self, File, Metadata},
    io,
    os::unix::{
        fs::{DirBuilderExt, FileTypeExt, MetadataExt, PermissionsExt},
        net::{SocketAddr, UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
};

use nix::{
    errno::Errno,
    fcntl::{Flock, FlockArg, OFlag, open},
    sys::stat::Mode,
    unistd::geteuid,
};
use podway_service::ServiceRuntimePathsV1;

const RUNTIME_DIRECTORY_MODE: u32 = 0o700;
const LOCK_FILE_MODE: u32 = 0o600;
const SOCKET_MODE: u32 = 0o600;

/// A stable identity for the socket that a guard created.
///
/// The guard compares this identity before unlinking at shutdown so it never removes a replacement
/// socket installed after its listener stopped owning the path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SocketOwnershipTokenV1 {
    device: u64,
    inode: u64,
    owner_uid: u32,
}

impl SocketOwnershipTokenV1 {
    pub const fn device(self) -> u64 {
        self.device
    }

    pub const fn inode(self) -> u64 {
        self.inode
    }

    pub const fn owner_uid(self) -> u32 {
        self.owner_uid
    }
}

/// The filesystem property that made a service-supplied path unsafe to use.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EndpointPathViolationV1 {
    Symlink,
    NotDirectory,
    NotRegularFile,
    NotSocket,
    ReplacedDuringSetup,
    WrongOwner {
        expected_uid: u32,
        actual_uid: u32,
    },
    WrongMode {
        expected_mode: u32,
        actual_mode: u32,
    },
}

impl fmt::Display for EndpointPathViolationV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Symlink => formatter.write_str("is a symlink"),
            Self::NotDirectory => formatter.write_str("is not a directory"),
            Self::NotRegularFile => formatter.write_str("is not a regular file"),
            Self::NotSocket => formatter.write_str("is not a Unix-domain socket"),
            Self::ReplacedDuringSetup => {
                formatter.write_str("was replaced while its ownership was being validated")
            }
            Self::WrongOwner {
                expected_uid,
                actual_uid,
            } => write!(
                formatter,
                "has owner UID {actual_uid}, expected current UID {expected_uid}"
            ),
            Self::WrongMode {
                expected_mode,
                actual_mode,
            } => write!(
                formatter,
                "has mode {actual_mode:o}, expected {expected_mode:o}"
            ),
        }
    }
}

/// Failures while acquiring or shutting down a daemon singleton endpoint.
#[derive(Debug)]
pub enum EndpointErrorV1 {
    AlreadyRunning,
    UnsafeRuntimeDirectory {
        path: PathBuf,
        violation: EndpointPathViolationV1,
    },
    UnsafeSocketParent {
        path: PathBuf,
        violation: EndpointPathViolationV1,
    },
    UnsafeLockFile {
        path: PathBuf,
        violation: EndpointPathViolationV1,
    },
    UnsafeSocket {
        path: PathBuf,
        violation: EndpointPathViolationV1,
    },
    Io {
        operation: &'static str,
        path: PathBuf,
        source: io::Error,
    },
    LockOpen {
        path: PathBuf,
        source: Errno,
    },
    LockAcquire {
        path: PathBuf,
        source: Errno,
    },
    LockRelease {
        path: PathBuf,
        source: Errno,
    },
    SocketProbe {
        path: PathBuf,
        source: io::Error,
    },
}

impl fmt::Display for EndpointErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyRunning => formatter.write_str("podwayd is already running for this user"),
            Self::UnsafeRuntimeDirectory { path, violation } => {
                write!(
                    formatter,
                    "runtime directory {} {violation}",
                    path.display()
                )
            }
            Self::UnsafeSocketParent { path, violation } => {
                write!(formatter, "socket parent {} {violation}", path.display())
            }
            Self::UnsafeLockFile { path, violation } => {
                write!(formatter, "lock file {} {violation}", path.display())
            }
            Self::UnsafeSocket { path, violation } => {
                write!(formatter, "socket {} {violation}", path.display())
            }
            Self::Io {
                operation,
                path,
                source,
            } => write!(formatter, "cannot {operation} {}: {source}", path.display()),
            Self::LockOpen { path, source } => {
                write!(
                    formatter,
                    "cannot open lock file {}: {source}",
                    path.display()
                )
            }
            Self::LockAcquire { path, source } => {
                write!(formatter, "cannot lock {}: {source}", path.display())
            }
            Self::LockRelease { path, source } => {
                write!(formatter, "cannot unlock {}: {source}", path.display())
            }
            Self::SocketProbe { path, source } => {
                write!(
                    formatter,
                    "cannot verify socket {}: {source}",
                    path.display()
                )
            }
        }
    }
}

impl Error for EndpointErrorV1 {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io { source, .. } | Self::SocketProbe { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Namespace for acquiring the per-user daemon singleton endpoint.
pub struct SingletonEndpointV1;

impl SingletonEndpointV1 {
    /// Acquires the singleton lock and binds the service-supplied Unix socket.
    ///
    /// A lock loser returns [`EndpointErrorV1::AlreadyRunning`] before inspecting or changing the
    /// socket path. A lock owner may only remove a socket after proving that it is a stale,
    /// same-user, mode-0600 Unix socket.
    pub fn acquire(
        paths: &ServiceRuntimePathsV1,
    ) -> Result<SingletonEndpointGuardV1, EndpointErrorV1> {
        let runtime_directory = paths.runtime_directory().as_path();
        let lock_path = paths.global_lock_path().as_path();
        let socket_path = paths.socket_path().as_path();
        let current_uid = geteuid().as_raw();

        ensure_runtime_directory(runtime_directory, current_uid)?;
        let lock_parent = lock_path
            .parent()
            .ok_or_else(|| EndpointErrorV1::UnsafeLockFile {
                path: lock_path.to_path_buf(),
                violation: EndpointPathViolationV1::NotRegularFile,
            })?;
        if lock_parent != runtime_directory {
            ensure_runtime_directory(lock_parent, current_uid)?;
        }
        let lock = open_and_lock(lock_path, current_uid)?;

        ensure_socket_parent(socket_path, runtime_directory, current_uid)?;
        prepare_socket_path(socket_path, current_uid)?;
        let (listener, socket_token) =
            bind_and_configure_socket(socket_path, current_uid, configure_bound_socket)?;

        Ok(SingletonEndpointGuardV1 {
            listener,
            lock,
            lock_path: lock_path.to_path_buf(),
            socket_path: socket_path.to_path_buf(),
            socket_token,
        })
    }
}

fn ensure_socket_parent(
    socket_path: &Path,
    runtime_directory: &Path,
    current_uid: u32,
) -> Result<(), EndpointErrorV1> {
    let parent = socket_path
        .parent()
        .ok_or_else(|| EndpointErrorV1::UnsafeSocketParent {
            path: socket_path.to_path_buf(),
            violation: EndpointPathViolationV1::NotDirectory,
        })?;
    if parent == runtime_directory {
        return Ok(());
    }
    let metadata = fs::symlink_metadata(parent).map_err(|source| EndpointErrorV1::Io {
        operation: "inspect socket parent",
        path: parent.to_path_buf(),
        source,
    })?;
    validate_private_directory(parent, &metadata, current_uid).map_err(|violation| {
        EndpointErrorV1::UnsafeSocketParent {
            path: parent.to_path_buf(),
            violation,
        }
    })
}

/// Owns a daemon listener, its exclusive process lock, and the socket identity used for cleanup.
pub struct SingletonEndpointGuardV1 {
    listener: UnixListener,
    lock: Flock<File>,
    lock_path: PathBuf,
    socket_path: PathBuf,
    socket_token: SocketOwnershipTokenV1,
}

impl SingletonEndpointGuardV1 {
    pub fn listener(&self) -> &UnixListener {
        &self.listener
    }

    pub fn socket_path(&self) -> &Path {
        &self.socket_path
    }

    pub const fn socket_ownership_token(&self) -> SocketOwnershipTokenV1 {
        self.socket_token
    }

    pub fn accept(&self) -> io::Result<(UnixStream, SocketAddr)> {
        self.listener.accept()
    }

    /// Closes the listener and removes its current socket while retaining the singleton lock.
    ///
    /// A path is removed only when it is still a socket with the device, inode, and owner captured
    /// at bind time. A replacement path is deliberately left untouched before the lock is released.
    pub fn shutdown(self) -> Result<(), EndpointErrorV1> {
        let Self {
            listener,
            lock,
            lock_path,
            socket_path,
            socket_token,
        } = self;

        release_endpoint(
            listener,
            lock,
            lock_path,
            socket_path,
            socket_token,
            remove_socket_if_current,
        )
    }
}

fn bind_and_configure_socket(
    path: &Path,
    current_uid: u32,
    configure_socket: impl FnOnce(
        &Path,
        &UnixListener,
        u32,
    ) -> Result<SocketOwnershipTokenV1, EndpointErrorV1>,
) -> Result<(UnixListener, SocketOwnershipTokenV1), EndpointErrorV1> {
    let listener = UnixListener::bind(path).map_err(|source| EndpointErrorV1::Io {
        operation: "bind socket",
        path: path.to_path_buf(),
        source,
    })?;
    let bound_token = socket_token_from_path(path)?;
    let mut provisional_cleanup = ProvisionalSocketCleanupGuardV1::new(path, bound_token);

    let socket_token = match configure_socket(path, &listener, current_uid) {
        Ok(socket_token) => socket_token,
        Err(error) => {
            let _ = provisional_cleanup.remove_if_current();
            return Err(error);
        }
    };
    if socket_token != bound_token {
        let _ = provisional_cleanup.remove_if_current();
        return Err(EndpointErrorV1::UnsafeSocket {
            path: path.to_path_buf(),
            violation: EndpointPathViolationV1::ReplacedDuringSetup,
        });
    }

    provisional_cleanup.disarm();
    Ok((listener, socket_token))
}

fn configure_bound_socket(
    path: &Path,
    _listener: &UnixListener,
    current_uid: u32,
) -> Result<SocketOwnershipTokenV1, EndpointErrorV1> {
    set_mode(path, SOCKET_MODE, "set socket permissions")?;
    validated_socket_token(path, current_uid)
}

fn release_endpoint(
    listener: UnixListener,
    lock: Flock<File>,
    lock_path: PathBuf,
    socket_path: PathBuf,
    socket_token: SocketOwnershipTokenV1,
    remove_socket: impl FnOnce(&Path, SocketOwnershipTokenV1) -> Result<(), EndpointErrorV1>,
) -> Result<(), EndpointErrorV1> {
    drop(listener);
    remove_socket(&socket_path, socket_token)?;

    let lock_file = Flock::unlock(lock).map_err(|(_, source)| EndpointErrorV1::LockRelease {
        path: lock_path,
        source,
    })?;
    drop(lock_file);

    Ok(())
}

/// Retains the bound pathname identity until post-bind setup succeeds.
///
/// If setup fails, cleanup compares the captured device and inode with the path before unlinking,
/// so a replacement installed during setup is never removed.
struct ProvisionalSocketCleanupGuardV1<'path> {
    path: &'path Path,
    token: SocketOwnershipTokenV1,
    armed: bool,
}

impl<'path> ProvisionalSocketCleanupGuardV1<'path> {
    fn new(path: &'path Path, token: SocketOwnershipTokenV1) -> Self {
        Self {
            path,
            token,
            armed: true,
        }
    }

    fn remove_if_current(&mut self) -> Result<(), EndpointErrorV1> {
        if !self.armed {
            return Ok(());
        }

        remove_socket_if_current(self.path, self.token)?;
        self.armed = false;
        Ok(())
    }

    fn disarm(mut self) {
        self.armed = false;
    }
}

impl Drop for ProvisionalSocketCleanupGuardV1<'_> {
    fn drop(&mut self) {
        let _ = self.remove_if_current();
    }
}

fn ensure_runtime_directory(path: &Path, current_uid: u32) -> Result<(), EndpointErrorV1> {
    match fs::symlink_metadata(path) {
        Ok(_) => {}
        Err(source) if source.kind() == io::ErrorKind::NotFound => {
            let mut builder = fs::DirBuilder::new();
            builder.mode(RUNTIME_DIRECTORY_MODE);

            match builder.create(path) {
                Ok(()) => {}
                Err(source) if source.kind() == io::ErrorKind::AlreadyExists => {}
                Err(source) => {
                    return Err(EndpointErrorV1::Io {
                        operation: "create runtime directory",
                        path: path.to_path_buf(),
                        source,
                    });
                }
            }
        }
        Err(source) => {
            return Err(EndpointErrorV1::Io {
                operation: "inspect runtime directory",
                path: path.to_path_buf(),
                source,
            });
        }
    }

    let metadata = fs::symlink_metadata(path).map_err(|source| EndpointErrorV1::Io {
        operation: "inspect runtime directory",
        path: path.to_path_buf(),
        source,
    })?;
    validate_runtime_directory(path, &metadata, current_uid)
}

fn open_and_lock(path: &Path, current_uid: u32) -> Result<Flock<File>, EndpointErrorV1> {
    let descriptor = open(
        path,
        OFlag::O_CLOEXEC | OFlag::O_CREAT | OFlag::O_NOFOLLOW | OFlag::O_RDWR,
        Mode::S_IRUSR | Mode::S_IWUSR,
    )
    .map_err(|source| EndpointErrorV1::LockOpen {
        path: path.to_path_buf(),
        source,
    })?;
    let file = File::from(descriptor);
    let metadata = file.metadata().map_err(|source| EndpointErrorV1::Io {
        operation: "inspect lock file",
        path: path.to_path_buf(),
        source,
    })?;
    validate_lock_file(path, &metadata, current_uid)?;

    Flock::lock(file, FlockArg::LockExclusiveNonblock).map_err(|(_, source)| {
        if source == Errno::EWOULDBLOCK {
            EndpointErrorV1::AlreadyRunning
        } else {
            EndpointErrorV1::LockAcquire {
                path: path.to_path_buf(),
                source,
            }
        }
    })
}

fn prepare_socket_path(path: &Path, current_uid: u32) -> Result<(), EndpointErrorV1> {
    let Some(token) = existing_socket_token(path, current_uid)? else {
        return Ok(());
    };

    match UnixStream::connect(path) {
        Ok(stream) => {
            drop(stream);
            Err(EndpointErrorV1::AlreadyRunning)
        }
        Err(source) if source.kind() == io::ErrorKind::ConnectionRefused => {
            remove_stale_socket(path, token)
        }
        Err(source) => Err(EndpointErrorV1::SocketProbe {
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn remove_stale_socket(path: &Path, token: SocketOwnershipTokenV1) -> Result<(), EndpointErrorV1> {
    if socket_matches_token(path, token)? {
        fs::remove_file(path).map_err(|source| EndpointErrorV1::Io {
            operation: "remove verified stale socket",
            path: path.to_path_buf(),
            source,
        })?;
    }
    Ok(())
}

fn remove_socket_if_current(
    path: &Path,
    token: SocketOwnershipTokenV1,
) -> Result<(), EndpointErrorV1> {
    if socket_matches_token(path, token)? {
        fs::remove_file(path).map_err(|source| EndpointErrorV1::Io {
            operation: "remove current socket",
            path: path.to_path_buf(),
            source,
        })?;
    }
    Ok(())
}

fn socket_matches_token(
    path: &Path,
    token: SocketOwnershipTokenV1,
) -> Result<bool, EndpointErrorV1> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(source) => {
            return Err(EndpointErrorV1::Io {
                operation: "inspect socket before removal",
                path: path.to_path_buf(),
                source,
            });
        }
    };

    Ok(metadata.file_type().is_socket()
        && metadata.uid() == token.owner_uid
        && metadata.dev() == token.device
        && metadata.ino() == token.inode)
}

fn existing_socket_token(
    path: &Path,
    current_uid: u32,
) -> Result<Option<SocketOwnershipTokenV1>, EndpointErrorV1> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            validated_socket_token_from_metadata(path, &metadata, current_uid).map(Some)
        }
        Err(source) if source.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(EndpointErrorV1::Io {
            operation: "inspect socket",
            path: path.to_path_buf(),
            source,
        }),
    }
}

fn validated_socket_token(
    path: &Path,
    current_uid: u32,
) -> Result<SocketOwnershipTokenV1, EndpointErrorV1> {
    let metadata = fs::symlink_metadata(path).map_err(|source| EndpointErrorV1::Io {
        operation: "inspect bound socket",
        path: path.to_path_buf(),
        source,
    })?;
    validated_socket_token_from_metadata(path, &metadata, current_uid)
}
fn socket_token_from_path(path: &Path) -> Result<SocketOwnershipTokenV1, EndpointErrorV1> {
    let metadata = fs::symlink_metadata(path).map_err(|source| EndpointErrorV1::Io {
        operation: "inspect bound socket",
        path: path.to_path_buf(),
        source,
    })?;

    Ok(SocketOwnershipTokenV1 {
        device: metadata.dev(),
        inode: metadata.ino(),
        owner_uid: metadata.uid(),
    })
}
fn validated_socket_token_from_metadata(
    path: &Path,
    metadata: &Metadata,
    current_uid: u32,
) -> Result<SocketOwnershipTokenV1, EndpointErrorV1> {
    validate_socket(path, metadata, current_uid)?;
    Ok(SocketOwnershipTokenV1 {
        device: metadata.dev(),
        inode: metadata.ino(),
        owner_uid: metadata.uid(),
    })
}

fn validate_runtime_directory(
    path: &Path,
    metadata: &Metadata,
    current_uid: u32,
) -> Result<(), EndpointErrorV1> {
    validate_private_directory(path, metadata, current_uid).map_err(|violation| {
        EndpointErrorV1::UnsafeRuntimeDirectory {
            path: path.to_path_buf(),
            violation,
        }
    })
}

fn validate_private_directory(
    _path: &Path,
    metadata: &Metadata,
    current_uid: u32,
) -> Result<(), EndpointPathViolationV1> {
    let violation = if metadata.file_type().is_symlink() {
        Some(EndpointPathViolationV1::Symlink)
    } else if !metadata.is_dir() {
        Some(EndpointPathViolationV1::NotDirectory)
    } else if metadata.uid() != current_uid {
        Some(EndpointPathViolationV1::WrongOwner {
            expected_uid: current_uid,
            actual_uid: metadata.uid(),
        })
    } else {
        let actual_mode = metadata.permissions().mode() & 0o777;
        (actual_mode != RUNTIME_DIRECTORY_MODE).then_some(EndpointPathViolationV1::WrongMode {
            expected_mode: RUNTIME_DIRECTORY_MODE,
            actual_mode,
        })
    };

    violation.map_or(Ok(()), Err)
}

fn validate_lock_file(
    path: &Path,
    metadata: &Metadata,
    current_uid: u32,
) -> Result<(), EndpointErrorV1> {
    let violation = if !metadata.file_type().is_file() {
        Some(EndpointPathViolationV1::NotRegularFile)
    } else if metadata.uid() != current_uid {
        Some(EndpointPathViolationV1::WrongOwner {
            expected_uid: current_uid,
            actual_uid: metadata.uid(),
        })
    } else {
        let actual_mode = metadata.permissions().mode() & 0o777;
        (actual_mode != LOCK_FILE_MODE).then_some(EndpointPathViolationV1::WrongMode {
            expected_mode: LOCK_FILE_MODE,
            actual_mode,
        })
    };

    violation.map_or(Ok(()), |violation| {
        Err(EndpointErrorV1::UnsafeLockFile {
            path: path.to_path_buf(),
            violation,
        })
    })
}

fn validate_socket(
    path: &Path,
    metadata: &Metadata,
    current_uid: u32,
) -> Result<(), EndpointErrorV1> {
    let violation = if metadata.file_type().is_symlink() {
        Some(EndpointPathViolationV1::Symlink)
    } else if !metadata.file_type().is_socket() {
        Some(EndpointPathViolationV1::NotSocket)
    } else if metadata.uid() != current_uid {
        Some(EndpointPathViolationV1::WrongOwner {
            expected_uid: current_uid,
            actual_uid: metadata.uid(),
        })
    } else {
        let actual_mode = metadata.permissions().mode() & 0o777;
        (actual_mode != SOCKET_MODE).then_some(EndpointPathViolationV1::WrongMode {
            expected_mode: SOCKET_MODE,
            actual_mode,
        })
    };

    violation.map_or(Ok(()), |violation| {
        Err(EndpointErrorV1::UnsafeSocket {
            path: path.to_path_buf(),
            violation,
        })
    })
}

fn set_mode(path: &Path, mode: u32, operation: &'static str) -> Result<(), EndpointErrorV1> {
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(|source| {
        EndpointErrorV1::Io {
            operation,
            path: path.to_path_buf(),
            source,
        }
    })
}

#[cfg(test)]
mod tests {
    use std::{
        cell::Cell,
        env, fs,
        fs::File,
        os::unix::{fs::MetadataExt, net::UnixListener},
        path::{Path, PathBuf},
        sync::atomic::{AtomicU64, Ordering},
    };

    use nix::{
        fcntl::{Flock, FlockArg},
        unistd::geteuid,
    };

    use super::{
        EndpointErrorV1, EndpointPathViolationV1, LOCK_FILE_MODE, SOCKET_MODE,
        SocketOwnershipTokenV1, bind_and_configure_socket, open_and_lock, release_endpoint,
        remove_socket_if_current, set_mode, validate_lock_file, validate_runtime_directory,
        validate_socket, validated_socket_token,
    };

    static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    fn temporary_directory() -> PathBuf {
        let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let directory = env::temp_dir().join(format!(
            "podway-daemon-endpoint-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&directory).expect("test directory must be created");
        directory
    }

    fn token_from_path(path: &Path) -> SocketOwnershipTokenV1 {
        let metadata = fs::symlink_metadata(path).expect("test socket must exist");
        SocketOwnershipTokenV1 {
            device: metadata.dev(),
            inode: metadata.ino(),
            owner_uid: metadata.uid(),
        }
    }

    #[test]
    fn aut_t_sock_wrong_owner_metadata_fails_closed_without_chown() {
        let directory = temporary_directory();
        set_mode(&directory, 0o700, "set test directory permissions")
            .expect("test directory must be owner-private");
        let lock_path = directory.join("endpoint.lock");
        let socket_path = directory.join("endpoint.sock");
        let lock_file = File::create(&lock_path).expect("test lock file must be created");
        set_mode(&lock_path, LOCK_FILE_MODE, "set test lock permissions")
            .expect("test lock must be owner-private");
        let listener = UnixListener::bind(&socket_path).expect("test socket must bind");
        set_mode(&socket_path, SOCKET_MODE, "set test socket permissions")
            .expect("test socket must be owner-private");

        let directory_metadata = fs::symlink_metadata(&directory).expect("directory metadata");
        let lock_metadata = fs::symlink_metadata(&lock_path).expect("lock metadata");
        let socket_metadata = fs::symlink_metadata(&socket_path).expect("socket metadata");
        let actual_uid = directory_metadata.uid();
        let expected_uid = actual_uid.wrapping_add(1);
        assert_ne!(expected_uid, actual_uid);
        assert_eq!(lock_metadata.uid(), actual_uid);
        assert_eq!(socket_metadata.uid(), actual_uid);

        assert!(matches!(
            validate_runtime_directory(&directory, &directory_metadata, expected_uid),
            Err(EndpointErrorV1::UnsafeRuntimeDirectory {
                violation: EndpointPathViolationV1::WrongOwner {
                    expected_uid: observed_expected,
                    actual_uid: observed_actual,
                },
                ..
            }) if observed_expected == expected_uid && observed_actual == actual_uid
        ));
        assert!(matches!(
            validate_lock_file(&lock_path, &lock_metadata, expected_uid),
            Err(EndpointErrorV1::UnsafeLockFile {
                violation: EndpointPathViolationV1::WrongOwner {
                    expected_uid: observed_expected,
                    actual_uid: observed_actual,
                },
                ..
            }) if observed_expected == expected_uid && observed_actual == actual_uid
        ));
        assert!(matches!(
            validate_socket(&socket_path, &socket_metadata, expected_uid),
            Err(EndpointErrorV1::UnsafeSocket {
                violation: EndpointPathViolationV1::WrongOwner {
                    expected_uid: observed_expected,
                    actual_uid: observed_actual,
                },
                ..
            }) if observed_expected == expected_uid && observed_actual == actual_uid
        ));

        drop(listener);
        drop(lock_file);
        fs::remove_file(&socket_path).expect("test socket must be removed");
        fs::remove_file(&lock_path).expect("test lock must be removed");
        fs::remove_dir(&directory).expect("test directory must be removed");
    }

    #[test]
    fn post_bind_failure_removes_only_the_socket_just_bound() {
        let directory = temporary_directory();
        let socket_path = directory.join("endpoint.sock");
        let replacement_token = Cell::new(None);

        let result = bind_and_configure_socket(
            &socket_path,
            geteuid().as_raw(),
            |path, _listener, _current_uid| {
                fs::remove_file(path).expect("bound socket must be removed for replacement");
                let replacement = UnixListener::bind(path).expect("replacement socket must bind");
                set_mode(path, SOCKET_MODE, "set replacement socket permissions")
                    .expect("replacement socket permissions must be set");
                replacement_token.set(Some(
                    validated_socket_token(path, geteuid().as_raw())
                        .expect("replacement socket identity must be captured"),
                ));
                drop(replacement);

                Err(EndpointErrorV1::UnsafeSocket {
                    path: path.to_path_buf(),
                    violation: EndpointPathViolationV1::WrongMode {
                        expected_mode: SOCKET_MODE,
                        actual_mode: 0,
                    },
                })
            },
        );

        assert!(matches!(
            result,
            Err(EndpointErrorV1::UnsafeSocket {
                violation: EndpointPathViolationV1::WrongMode { .. },
                ..
            })
        ));
        assert_eq!(
            token_from_path(&socket_path),
            replacement_token
                .get()
                .expect("replacement identity must be retained")
        );

        fs::remove_file(&socket_path).expect("replacement socket must be removed");
        fs::remove_dir(&directory).expect("test directory must be removed");
    }

    #[test]
    fn endpoint_cleanup_keeps_the_lock_through_removal_and_preserves_a_replacement() {
        let directory = temporary_directory();
        let lock_path = directory.join("endpoint.lock");
        let socket_path = directory.join("endpoint.sock");
        let lock_file = File::create(&lock_path).expect("lock file must be created");
        set_mode(&lock_path, LOCK_FILE_MODE, "set lock file permissions")
            .expect("lock file permissions must be set");
        let lock = Flock::lock(lock_file, FlockArg::LockExclusiveNonblock)
            .expect("test lock must be acquired");
        let listener = UnixListener::bind(&socket_path).expect("socket must bind");
        set_mode(&socket_path, SOCKET_MODE, "set socket permissions")
            .expect("socket permissions must be set");
        let socket_token = validated_socket_token(&socket_path, geteuid().as_raw())
            .expect("bound socket identity must be captured");
        let replacement_token = Cell::new(None);

        release_endpoint(
            listener,
            lock,
            lock_path.clone(),
            socket_path.clone(),
            socket_token,
            |path, token| {
                assert!(matches!(
                    open_and_lock(&lock_path, geteuid().as_raw()),
                    Err(EndpointErrorV1::AlreadyRunning)
                ));

                remove_socket_if_current(path, token)?;
                let replacement = UnixListener::bind(path).expect("replacement socket must bind");
                set_mode(path, SOCKET_MODE, "set replacement socket permissions")
                    .expect("replacement socket permissions must be set");
                replacement_token.set(Some(
                    validated_socket_token(path, geteuid().as_raw())
                        .expect("replacement socket identity must be captured"),
                ));
                drop(replacement);

                remove_socket_if_current(path, token)
            },
        )
        .expect("endpoint release must preserve the replacement");

        assert_eq!(
            token_from_path(&socket_path),
            replacement_token
                .get()
                .expect("replacement identity must be retained")
        );

        fs::remove_file(&socket_path).expect("replacement socket must be removed");
        fs::remove_file(&lock_path).expect("lock file must be removed");
        fs::remove_dir(&directory).expect("test directory must be removed");
    }
}
