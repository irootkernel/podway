//! Phase 4 singleton endpoint and peer UID boundary contracts.

#![forbid(unsafe_code)]

use nix::unistd::geteuid;
use std::{
    cell::Cell,
    env, fs,
    os::unix::{
        fs::{MetadataExt, PermissionsExt, symlink},
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

use podway_daemon::endpoint::{EndpointErrorV1, EndpointPathViolationV1, SingletonEndpointV1};
use podway_daemon::peer::{
    FixedPeerCredentialSourceV1, PeerCredentialErrorV1, PeerFrameAdmissionErrorV1,
    PeerUidVerificationErrorV1, PeerUidVerifierV1,
};
use podway_service::ServiceRuntimePathsV1;

static FIXTURE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct RuntimeFixture {
    root: PathBuf,
    paths: ServiceRuntimePathsV1,
}

impl RuntimeFixture {
    fn new() -> Self {
        let sequence = FIXTURE_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let root = env::temp_dir().join(format!(
            "podway-daemon-phase4-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&root).expect("fixture root must be created");

        let launch_agents = root.join("LaunchAgents");
        let application_support = root.join("ApplicationSupport");
        let logs = root.join("Logs");
        let runtime = root.join("Runtime");
        for directory in [&launch_agents, &application_support, &logs] {
            fs::create_dir(directory).expect("fixture service directory must be created");
        }

        let paths = ServiceRuntimePathsV1::from_directories(
            launch_agents,
            application_support,
            logs,
            runtime,
        )
        .expect("fixture paths must be valid service paths");
        Self { root, paths }
    }

    fn runtime_directory(&self) -> &Path {
        self.paths.runtime_directory().as_path()
    }

    fn socket_path(&self) -> &Path {
        self.paths.socket_path().as_path()
    }

    fn lock_path(&self) -> &Path {
        self.paths.global_lock_path().as_path()
    }
}

impl Drop for RuntimeFixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn mode(path: &Path) -> u32 {
    fs::symlink_metadata(path)
        .expect("fixture path must exist")
        .permissions()
        .mode()
        & 0o777
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SocketIdentity {
    device: u64,
    inode: u64,
    owner_uid: u32,
}

fn ownership_token(path: &Path) -> SocketIdentity {
    let metadata = fs::symlink_metadata(path).expect("socket fixture must exist");
    SocketIdentity {
        device: metadata.dev(),
        inode: metadata.ino(),
        owner_uid: metadata.uid(),
    }
}

fn set_mode(path: &Path, mode: u32) {
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .expect("fixture permissions must be set");
}

fn stale_socket(path: &Path, mode: u32) -> SocketIdentity {
    let listener = UnixListener::bind(path).expect("stale Unix socket must bind");
    set_mode(path, mode);
    let token = ownership_token(path);
    drop(listener);
    token
}

fn connected_pair(directory: &Path) -> (UnixListener, UnixStream, UnixStream, PathBuf) {
    let path = directory.join("peer.sock");
    let listener = UnixListener::bind(&path).expect("peer listener must bind");
    let client = UnixStream::connect(&path).expect("client must connect to peer listener");
    let (server, _) = listener.accept().expect("listener must accept peer client");
    (listener, client, server, path)
}

#[test]
fn singleton_loser_never_unlinks_the_live_socket() {
    let fixture = RuntimeFixture::new();
    let owner = SingletonEndpointV1::acquire(&fixture.paths).expect("first daemon owns endpoint");
    let before = ownership_token(fixture.socket_path());

    assert!(matches!(
        SingletonEndpointV1::acquire(&fixture.paths),
        Err(EndpointErrorV1::AlreadyRunning)
    ));
    assert_eq!(ownership_token(fixture.socket_path()), before);

    owner.shutdown().expect("owner must shut down cleanly");
}

#[test]
fn verified_stale_socket_is_recovered_after_connect_refusal() {
    let fixture = RuntimeFixture::new();
    fs::create_dir(fixture.runtime_directory()).expect("runtime directory fixture must be created");
    set_mode(fixture.runtime_directory(), 0o700);
    let stale = stale_socket(fixture.socket_path(), 0o600);

    let owner =
        SingletonEndpointV1::acquire(&fixture.paths).expect("stale socket must be recovered");
    let replacement = ownership_token(fixture.socket_path());
    assert_ne!(replacement, stale, "recovery must bind a new socket object");

    owner
        .shutdown()
        .expect("replacement socket must shut down cleanly");
}

#[test]
fn unsafe_runtime_lock_and_socket_paths_fail_closed() {
    let runtime_fixture = RuntimeFixture::new();
    fs::create_dir(runtime_fixture.runtime_directory()).expect("runtime fixture must be created");
    set_mode(runtime_fixture.runtime_directory(), 0o755);
    assert!(matches!(
        SingletonEndpointV1::acquire(&runtime_fixture.paths),
        Err(EndpointErrorV1::UnsafeRuntimeDirectory {
            violation: EndpointPathViolationV1::WrongMode { .. },
            ..
        })
    ));

    let symlink_fixture = RuntimeFixture::new();
    fs::create_dir(symlink_fixture.runtime_directory())
        .expect("runtime directory fixture must be created");
    set_mode(symlink_fixture.runtime_directory(), 0o700);
    fs::write(symlink_fixture.root.join("target"), "not a socket")
        .expect("symlink target fixture must be created");
    symlink(
        symlink_fixture.root.join("target"),
        symlink_fixture.socket_path(),
    )
    .expect("socket symlink fixture must be created");
    assert!(matches!(
        SingletonEndpointV1::acquire(&symlink_fixture.paths),
        Err(EndpointErrorV1::UnsafeSocket {
            violation: EndpointPathViolationV1::Symlink,
            ..
        })
    ));

    let regular_fixture = RuntimeFixture::new();
    fs::create_dir(regular_fixture.runtime_directory())
        .expect("runtime directory fixture must be created");
    set_mode(regular_fixture.runtime_directory(), 0o700);
    fs::write(regular_fixture.socket_path(), "not a socket")
        .expect("regular socket-path fixture must be created");
    assert!(matches!(
        SingletonEndpointV1::acquire(&regular_fixture.paths),
        Err(EndpointErrorV1::UnsafeSocket {
            violation: EndpointPathViolationV1::NotSocket,
            ..
        })
    ));

    let socket_mode_fixture = RuntimeFixture::new();
    fs::create_dir(socket_mode_fixture.runtime_directory())
        .expect("runtime directory fixture must be created");
    set_mode(socket_mode_fixture.runtime_directory(), 0o700);
    let _stale = stale_socket(socket_mode_fixture.socket_path(), 0o660);
    assert!(matches!(
        SingletonEndpointV1::acquire(&socket_mode_fixture.paths),
        Err(EndpointErrorV1::UnsafeSocket {
            violation: EndpointPathViolationV1::WrongMode { .. },
            ..
        })
    ));

    let lock_mode_fixture = RuntimeFixture::new();
    fs::create_dir(lock_mode_fixture.runtime_directory())
        .expect("runtime directory fixture must be created");
    set_mode(lock_mode_fixture.runtime_directory(), 0o700);
    fs::write(lock_mode_fixture.lock_path(), "lock fixture").expect("lock fixture must be created");
    set_mode(lock_mode_fixture.lock_path(), 0o640);
    assert!(matches!(
        SingletonEndpointV1::acquire(&lock_mode_fixture.paths),
        Err(EndpointErrorV1::UnsafeLockFile {
            violation: EndpointPathViolationV1::WrongMode { .. },
            ..
        })
    ));
}

#[test]
fn endpoint_creates_private_runtime_lock_and_socket_modes() {
    let fixture = RuntimeFixture::new();
    let owner = SingletonEndpointV1::acquire(&fixture.paths).expect("endpoint must be acquired");

    assert_eq!(mode(fixture.runtime_directory()), 0o700);
    assert_eq!(mode(fixture.lock_path()), 0o600);
    assert_eq!(mode(fixture.socket_path()), 0o600);
    assert_eq!(
        owner.socket_ownership_token().owner_uid(),
        geteuid().as_raw(),
        "endpoint ownership must use the daemon effective UID"
    );

    owner.shutdown().expect("owner must shut down cleanly");
}

#[test]
fn replacement_socket_survives_old_guard_shutdown() {
    let fixture = RuntimeFixture::new();
    let owner = SingletonEndpointV1::acquire(&fixture.paths).expect("endpoint must be acquired");
    let old_token = owner.socket_ownership_token();

    fs::remove_file(fixture.socket_path())
        .expect("old socket path must be unlinked for replacement");
    let replacement_listener = UnixListener::bind(fixture.socket_path())
        .expect("replacement socket must bind after old path is unlinked");
    set_mode(fixture.socket_path(), 0o600);
    let replacement_token = ownership_token(fixture.socket_path());
    assert_ne!(
        replacement_token,
        SocketIdentity {
            device: old_token.device(),
            inode: old_token.inode(),
            owner_uid: old_token.owner_uid(),
        }
    );

    owner
        .shutdown()
        .expect("old guard shutdown must preserve replacement");
    assert_eq!(ownership_token(fixture.socket_path()), replacement_token);

    drop(replacement_listener);
    fs::remove_file(fixture.socket_path()).expect("replacement fixture socket must be removed");
}

#[cfg(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly",
    target_os = "linux"
))]
#[test]
fn native_same_effective_uid_peer_is_accepted() {
    let fixture = RuntimeFixture::new();
    let (listener, client, server, socket_path) = connected_pair(&fixture.root);
    let verifier = PeerUidVerifierV1::for_current_user();
    assert_eq!(
        verifier.expected_uid(),
        geteuid().as_raw(),
        "native peer verification must use the daemon effective UID"
    );

    verifier
        .verify(&server)
        .expect("same-user Unix peer must pass native credential verification");

    drop(client);
    drop(server);
    drop(listener);
    fs::remove_file(socket_path).expect("peer socket fixture must be removed");
}

#[cfg(not(any(
    target_os = "macos",
    target_os = "ios",
    target_os = "freebsd",
    target_os = "openbsd",
    target_os = "netbsd",
    target_os = "dragonfly",
    target_os = "linux"
)))]
#[test]
fn native_peer_credentials_are_explicitly_unsupported_on_other_targets() {
    let fixture = RuntimeFixture::new();
    let (listener, client, server, socket_path) = connected_pair(&fixture.root);
    let verifier = PeerUidVerifierV1::for_current_user();

    assert!(matches!(
        verifier.verify(&server),
        Err(PeerUidVerificationErrorV1::Credential(
            PeerCredentialErrorV1::UnsupportedPlatform
        ))
    ));

    drop(client);
    drop(server);
    drop(listener);
    fs::remove_file(socket_path).expect("peer socket fixture must be removed");
}

#[test]
fn injected_mismatch_and_credential_failure_reject_before_frame_reads() {
    let fixture = RuntimeFixture::new();
    let (listener, client, server, socket_path) = connected_pair(&fixture.root);
    let frame_reads = Cell::new(0_u8);
    let mismatch = PeerUidVerifierV1::new(501, FixedPeerCredentialSourceV1::uid(502));

    assert!(matches!(
        mismatch.verify_before_frame(&server, |_| {
            frame_reads.set(frame_reads.get() + 1);
            Ok(())
        }),
        Err(PeerFrameAdmissionErrorV1::Peer(
            PeerUidVerificationErrorV1::UidMismatch {
                expected_uid: 501,
                actual_uid: 502
            }
        ))
    ));
    assert_eq!(
        frame_reads.get(),
        0,
        "mismatched peer must not reach the frame reader"
    );

    let failure = PeerUidVerifierV1::new(
        501,
        FixedPeerCredentialSourceV1::failure(PeerCredentialErrorV1::UnsupportedPlatform),
    );
    assert!(matches!(
        failure.verify_before_frame(&server, |_| {
            frame_reads.set(frame_reads.get() + 1);
            Ok(())
        }),
        Err(PeerFrameAdmissionErrorV1::Peer(
            PeerUidVerificationErrorV1::Credential(PeerCredentialErrorV1::UnsupportedPlatform)
        ))
    ));
    assert_eq!(
        frame_reads.get(),
        0,
        "credential failure must not reach the frame reader"
    );

    drop(client);
    drop(server);
    drop(listener);
    fs::remove_file(socket_path).expect("peer socket fixture must be removed");
}
