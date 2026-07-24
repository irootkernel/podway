//! Synchronous client for the daemon-owned local IPC socket.

use std::{
    error::Error,
    fmt, fs,
    io::{self, Read, Write},
    os::unix::{
        fs::{FileTypeExt, MetadataExt, PermissionsExt},
        net::UnixStream,
    },
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

use nix::unistd::geteuid;
use podway_protocol::{
    FrameErrorV1, OperationV1, PayloadCodecErrorV1, RequestEnvelopeV1, ResponseEnvelopeV1,
    SliceErrorV1, SliceRequestV1, decode_response_payload_v1, encode_request_payload_v1,
    read_single_frame_v1, write_frame_v1,
};
use podway_service::ServiceRuntimePathsV1;

/// The default upper bound for establishing a daemon IPC connection.
pub const DEFAULT_DAEMON_CONNECT_TIMEOUT_V1: Duration = Duration::from_secs(5);
/// The default upper bound for writing one daemon IPC request frame.
pub const DEFAULT_DAEMON_WRITE_TIMEOUT_V1: Duration = Duration::from_secs(5);
/// The default upper bound for reading one daemon IPC response frame.
pub const DEFAULT_DAEMON_READ_TIMEOUT_V1: Duration = Duration::from_secs(5);

/// The local I/O operation associated with a client transport failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DaemonClientIoOperationV1 {
    Connect,
    ConfigureReadTimeout,
    ConfigureWriteTimeout,
    Write,
    ShutdownWrite,
    Read,
}

impl fmt::Display for DaemonClientIoOperationV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let operation = match self {
            Self::Connect => "connect",
            Self::ConfigureReadTimeout => "configure read timeout",
            Self::ConfigureWriteTimeout => "configure write timeout",
            Self::Write => "write request",
            Self::ShutdownWrite => "close request stream",
            Self::Read => "read response",
        };
        formatter.write_str(operation)
    }
}

/// Bounded timeouts for one daemon request/response exchange.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DaemonClientTimeoutsV1 {
    connect: Duration,
    read: Duration,
    write: Duration,
}

impl DaemonClientTimeoutsV1 {
    /// Creates non-zero bounds for connecting and exchanging a single frame in each direction.
    pub fn new(
        connect: Duration,
        read: Duration,
        write: Duration,
    ) -> Result<Self, DaemonClientErrorV1> {
        for (operation, timeout) in [
            (DaemonClientIoOperationV1::Connect, connect),
            (DaemonClientIoOperationV1::Read, read),
            (DaemonClientIoOperationV1::Write, write),
        ] {
            if timeout.is_zero() {
                return Err(DaemonClientErrorV1::InvalidTimeout { operation });
            }
        }
        Ok(Self {
            connect,
            read,
            write,
        })
    }

    /// Returns the bound for establishing a local socket connection.
    pub const fn connect(self) -> Duration {
        self.connect
    }

    /// Returns the bound for reading the daemon's response frame.
    pub const fn read(self) -> Duration {
        self.read
    }

    /// Returns the bound for writing the request frame.
    pub const fn write(self) -> Duration {
        self.write
    }
}

impl Default for DaemonClientTimeoutsV1 {
    fn default() -> Self {
        Self {
            connect: DEFAULT_DAEMON_CONNECT_TIMEOUT_V1,
            read: DEFAULT_DAEMON_READ_TIMEOUT_V1,
            write: DEFAULT_DAEMON_WRITE_TIMEOUT_V1,
        }
    }
}

/// Failures returned by the synchronous daemon client.
#[derive(Debug)]
pub enum DaemonClientErrorV1 {
    /// A caller provided a zero-duration transport bound.
    InvalidTimeout {
        operation: DaemonClientIoOperationV1,
    },
    /// The client could not connect to or retain the daemon socket.
    Connection {
        operation: DaemonClientIoOperationV1,
        source: io::Error,
    },
    /// Applying a valid local timeout to the socket failed.
    SocketConfiguration {
        operation: DaemonClientIoOperationV1,
        source: io::Error,
    },
    /// The selected endpoint is not an owner-private Unix socket in an owner-private directory.
    EndpointSecurity { message: String },
    /// The connected daemon process does not have the client's effective UID.
    PeerIdentity { expected_uid: u32, actual_uid: u32 },
    /// The client's bounded local I/O operation expired.
    Timeout {
        operation: DaemonClientIoOperationV1,
    },
    /// The request is outside the deliberately small G005 command slice.
    RequestAdmission { source: SliceErrorV1 },
    /// Encoding the outbound request envelope failed.
    RequestEncoding { source: PayloadCodecErrorV1 },
    /// Decoding the inbound response envelope failed.
    ResponseDecoding { source: PayloadCodecErrorV1 },
    /// The response did not contain exactly one valid v1 frame.
    Framing { source: FrameErrorV1 },
    /// The daemon closed the connection before producing a response frame.
    MissingResponse,
    /// The response did not correlate to the request sent on this connection.
    ResponseMismatch {
        field: &'static str,
        expected: String,
        received: String,
    },
}

impl fmt::Display for DaemonClientErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidTimeout { operation } => {
                write!(formatter, "daemon {operation} timeout must be non-zero")
            }
            Self::Connection { operation, source } => {
                write!(formatter, "daemon {operation} failed: {source}")
            }
            Self::SocketConfiguration { operation, source } => {
                write!(formatter, "cannot {operation} for daemon socket: {source}")
            }
            Self::EndpointSecurity { message } => {
                write!(formatter, "daemon endpoint is unsafe: {message}")
            }
            Self::PeerIdentity {
                expected_uid,
                actual_uid,
            } => write!(
                formatter,
                "daemon peer UID {actual_uid} does not match client UID {expected_uid}"
            ),
            Self::Timeout { operation } => write!(formatter, "daemon {operation} timed out"),
            Self::RequestAdmission { source } => {
                write!(formatter, "daemon request is invalid: {source}")
            }
            Self::RequestEncoding { source } => {
                write!(formatter, "daemon request encoding failed: {source}")
            }
            Self::ResponseDecoding { source } => {
                write!(formatter, "daemon response decoding failed: {source}")
            }
            Self::Framing { source } => write!(formatter, "daemon framing failed: {source}"),
            Self::MissingResponse => formatter.write_str("daemon closed without a response frame"),
            Self::ResponseMismatch {
                field,
                expected,
                received,
            } => write!(
                formatter,
                "daemon response {field} mismatch: expected {expected:?}, received {received:?}"
            ),
        }
    }
}

impl Error for DaemonClientErrorV1 {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Connection { source, .. } | Self::SocketConfiguration { source, .. } => {
                Some(source)
            }
            Self::RequestAdmission { source } => Some(source),
            Self::RequestEncoding { source } | Self::ResponseDecoding { source } => Some(source),
            Self::Framing { source } => Some(source),
            Self::InvalidTimeout { .. }
            | Self::EndpointSecurity { .. }
            | Self::PeerIdentity { .. }
            | Self::Timeout { .. }
            | Self::MissingResponse
            | Self::ResponseMismatch { .. } => None,
        }
    }
}

/// A synchronous client restricted to the service-owned local Unix socket.
#[derive(Clone, Debug)]
pub struct DaemonClientV1 {
    runtime_paths: ServiceRuntimePathsV1,
    timeouts: DaemonClientTimeoutsV1,
}

impl DaemonClientV1 {
    /// Creates a client using the service-owned runtime socket and default bounded timeouts.
    pub fn new(runtime_paths: ServiceRuntimePathsV1) -> Self {
        Self {
            runtime_paths,
            timeouts: DaemonClientTimeoutsV1::default(),
        }
    }

    /// Creates a client using the service-owned runtime socket and explicit bounded timeouts.
    pub fn with_timeouts(
        runtime_paths: ServiceRuntimePathsV1,
        timeouts: DaemonClientTimeoutsV1,
    ) -> Self {
        Self {
            runtime_paths,
            timeouts,
        }
    }

    /// Returns the transport bounds applied to each daemon exchange.
    pub const fn timeouts(&self) -> DaemonClientTimeoutsV1 {
        self.timeouts
    }

    /// Exchanges exactly one admitted G005 request and one daemon response.
    ///
    /// A timeout only abandons this local connection. It does not send cancellation and therefore
    /// never implies cancellation of a durably admitted daemon job.
    pub fn request(
        &self,
        request: &RequestEnvelopeV1,
    ) -> Result<ResponseEnvelopeV1, DaemonClientErrorV1> {
        SliceRequestV1::from_envelope(request)
            .map_err(|source| DaemonClientErrorV1::RequestAdmission { source })?;
        self.exchange(request)
    }

    /// Exchanges the exact read-only daemon process status probe outside the durable command slice.
    pub fn daemon_status(
        &self,
        request: &RequestEnvelopeV1,
    ) -> Result<ResponseEnvelopeV1, DaemonClientErrorV1> {
        if request.command().as_str() != "daemon.status"
            || request.operation() != OperationV1::Control
            || request.workspace().is_some()
            || request.idempotency_key().is_some()
            || request.preconditions().session_id().is_some()
            || request.preconditions().session_revision().is_some()
            || request.preconditions().attempt_id().is_some()
            || request.preconditions().item_revision().is_some()
            || request.preconditions().blocker_id().is_some()
            || request.preconditions().job_state().is_some()
            || request.options().detach()
            || request.options().wait_timeout_ms() != 0
            || !request.payload().is_empty()
        {
            return Err(DaemonClientErrorV1::RequestAdmission {
                source: SliceErrorV1::InvalidCommand {
                    received: request.command().as_str().to_owned(),
                },
            });
        }
        self.exchange(request)
    }

    fn exchange(
        &self,
        request: &RequestEnvelopeV1,
    ) -> Result<ResponseEnvelopeV1, DaemonClientErrorV1> {
        let payload = encode_request_payload_v1(request)
            .map_err(|source| DaemonClientErrorV1::RequestEncoding { source })?;

        let socket_path = self.runtime_paths.socket_path().as_path();
        validate_endpoint_path(socket_path)?;
        let mut stream = connect_with_timeout(socket_path.to_path_buf(), self.timeouts.connect())?;
        validate_peer_uid(&stream)?;
        stream
            .set_write_timeout(Some(self.timeouts.write()))
            .map_err(|source| DaemonClientErrorV1::SocketConfiguration {
                operation: DaemonClientIoOperationV1::ConfigureWriteTimeout,
                source,
            })?;
        stream
            .set_read_timeout(Some(self.timeouts.read()))
            .map_err(|source| DaemonClientErrorV1::SocketConfiguration {
                operation: DaemonClientIoOperationV1::ConfigureReadTimeout,
                source,
            })?;

        let write_deadline = ExchangeDeadlineV1::new(self.timeouts.write());
        {
            let mut writer = DeadlineStreamV1::for_write(&mut stream, write_deadline);
            write_frame_v1(&mut writer, &payload)
                .map_err(|error| map_frame_error(error, DaemonClientIoOperationV1::Write))?;
        }
        write_deadline
            .check()
            .map_err(|source| map_io_error(DaemonClientIoOperationV1::Write, source))?;
        stream
            .shutdown(std::net::Shutdown::Write)
            .map_err(|source| map_io_error(DaemonClientIoOperationV1::ShutdownWrite, source))?;

        let read_deadline = ExchangeDeadlineV1::new(self.timeouts.read());
        let response = {
            let mut reader = DeadlineStreamV1::for_read(&mut stream, read_deadline);
            read_single_frame_v1(&mut reader)
                .map_err(|error| map_frame_error(error, DaemonClientIoOperationV1::Read))?
                .ok_or(DaemonClientErrorV1::MissingResponse)?
        };
        let response = decode_response_payload_v1(&response)
            .map_err(|source| DaemonClientErrorV1::ResponseDecoding { source })?;
        validate_response_correlation(request, &response)?;
        Ok(response)
    }
}

fn validate_endpoint_path(socket_path: &Path) -> Result<(), DaemonClientErrorV1> {
    let expected_uid = geteuid().as_raw();
    let parent = socket_path
        .parent()
        .ok_or_else(|| DaemonClientErrorV1::EndpointSecurity {
            message: "socket path has no parent directory".to_owned(),
        })?;
    let parent_metadata =
        fs::symlink_metadata(parent).map_err(|source| DaemonClientErrorV1::EndpointSecurity {
            message: format!("cannot inspect socket parent: {source}"),
        })?;
    validate_socket_parent_metadata(&parent_metadata, expected_uid)?;

    let socket_metadata = match fs::symlink_metadata(socket_path) {
        Ok(metadata) => metadata,
        Err(source) if source.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(DaemonClientErrorV1::EndpointSecurity {
                message: format!("cannot inspect socket: {source}"),
            });
        }
    };
    validate_socket_metadata(&socket_metadata, expected_uid)
}

fn validate_socket_parent_metadata(
    metadata: &fs::Metadata,
    expected_uid: u32,
) -> Result<(), DaemonClientErrorV1> {
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(DaemonClientErrorV1::EndpointSecurity {
            message: "socket parent is not a real directory".to_owned(),
        });
    }
    let parent_mode = metadata.permissions().mode() & 0o777;
    if metadata.uid() != expected_uid || parent_mode != 0o700 {
        return Err(DaemonClientErrorV1::EndpointSecurity {
            message: format!("socket parent must be owned by UID {expected_uid} with mode 700"),
        });
    }
    Ok(())
}

fn validate_socket_metadata(
    metadata: &fs::Metadata,
    expected_uid: u32,
) -> Result<(), DaemonClientErrorV1> {
    let socket_mode = metadata.permissions().mode() & 0o777;
    if !metadata.file_type().is_socket() || metadata.uid() != expected_uid || socket_mode != 0o600 {
        return Err(DaemonClientErrorV1::EndpointSecurity {
            message: format!(
                "socket must be a Unix socket owned by UID {expected_uid} with mode 600"
            ),
        });
    }
    Ok(())
}

fn validate_peer_uid(stream: &UnixStream) -> Result<(), DaemonClientErrorV1> {
    let expected_uid = geteuid().as_raw();
    let actual_uid =
        native_peer_uid(stream).map_err(|source| DaemonClientErrorV1::EndpointSecurity {
            message: format!("cannot obtain daemon peer credentials: {source}"),
        })?;
    if actual_uid != expected_uid {
        return Err(DaemonClientErrorV1::PeerIdentity {
            expected_uid,
            actual_uid,
        });
    }
    Ok(())
}

fn native_peer_uid(stream: &UnixStream) -> io::Result<u32> {
    #[cfg(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "freebsd",
        target_os = "openbsd",
        target_os = "netbsd",
        target_os = "dragonfly"
    ))]
    {
        nix::unistd::getpeereid(stream)
            .map(|(uid, _)| uid.as_raw())
            .map_err(|error| io::Error::from_raw_os_error(error as i32))
    }

    #[cfg(target_os = "linux")]
    {
        nix::sys::socket::getsockopt(stream, nix::sys::socket::sockopt::PeerCredentials)
            .map(|credentials| credentials.uid())
            .map_err(|error| io::Error::from_raw_os_error(error as i32))
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
    {
        let _ = stream;
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "Unix peer credentials are unsupported on this platform",
        ))
    }
}

/// A monotonic deadline shared by every syscall in one framed direction.
#[derive(Clone, Copy, Debug)]
struct ExchangeDeadlineV1 {
    instant: Instant,
}

impl ExchangeDeadlineV1 {
    fn new(timeout: Duration) -> Self {
        Self {
            instant: Instant::now() + timeout,
        }
    }

    fn check(self) -> io::Result<()> {
        self.remaining().map(|_| ())
    }

    fn remaining(self) -> io::Result<Duration> {
        self.instant
            .checked_duration_since(Instant::now())
            .filter(|remaining| !remaining.is_zero())
            .ok_or_else(|| {
                io::Error::new(io::ErrorKind::TimedOut, "daemon exchange deadline elapsed")
            })
    }
}

/// The deadline direction used to reconfigure the socket with its remaining whole-exchange bound.
#[derive(Clone, Copy, Debug)]
enum DeadlineDirectionV1 {
    Read,
    Write,
}

impl DeadlineDirectionV1 {
    const fn configuration_operation(self) -> DaemonClientIoOperationV1 {
        match self {
            Self::Read => DaemonClientIoOperationV1::ConfigureReadTimeout,
            Self::Write => DaemonClientIoOperationV1::ConfigureWriteTimeout,
        }
    }
}

#[derive(Debug)]
struct DeadlineSocketConfigurationV1 {
    operation: DaemonClientIoOperationV1,
    timeout: Duration,
    source: io::Error,
}

impl fmt::Display for DeadlineSocketConfigurationV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "cannot {} with remaining timeout {:?}: {}",
            self.operation, self.timeout, self.source
        )
    }
}

impl Error for DeadlineSocketConfigurationV1 {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.source)
    }
}

/// A stream view that applies the remaining absolute deadline before every framed I/O syscall.
struct DeadlineStreamV1<'a> {
    stream: &'a mut UnixStream,
    deadline: ExchangeDeadlineV1,
    direction: DeadlineDirectionV1,
}

impl<'a> DeadlineStreamV1<'a> {
    fn for_read(stream: &'a mut UnixStream, deadline: ExchangeDeadlineV1) -> Self {
        Self {
            stream,
            deadline,
            direction: DeadlineDirectionV1::Read,
        }
    }

    fn for_write(stream: &'a mut UnixStream, deadline: ExchangeDeadlineV1) -> Self {
        Self {
            stream,
            deadline,
            direction: DeadlineDirectionV1::Write,
        }
    }

    fn prepare_io(&mut self) -> io::Result<()> {
        let timeout = self.deadline.remaining()?;
        let operation = self.direction.configuration_operation();
        let configured = match self.direction {
            DeadlineDirectionV1::Read => self.stream.set_read_timeout(Some(timeout)),
            DeadlineDirectionV1::Write => self.stream.set_write_timeout(Some(timeout)),
        };
        match configured {
            Ok(()) => Ok(()),
            Err(source) if source.kind() == io::ErrorKind::InvalidInput => {
                // Darwin rejects timeout reconfiguration after the peer has closed. The full
                // direction timeout was installed immediately after connect, and a closed peer
                // makes the following I/O return buffered bytes or EOF without blocking.
                Ok(())
            }
            Err(source) => {
                let kind = source.kind();
                Err(io::Error::new(
                    kind,
                    DeadlineSocketConfigurationV1 {
                        operation,
                        timeout,
                        source,
                    },
                ))
            }
        }
    }
}

impl Read for DeadlineStreamV1<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.prepare_io()?;
        self.stream.read(buffer)
    }
}

impl Write for DeadlineStreamV1<'_> {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.prepare_io()?;
        self.stream.write(buffer)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.prepare_io()?;
        self.stream.flush()
    }
}
fn connect_with_timeout(
    socket_path: PathBuf,
    timeout: Duration,
) -> Result<UnixStream, DaemonClientErrorV1> {
    let (sender, receiver) = mpsc::sync_channel(1);
    thread::spawn(move || {
        let _ = sender.send(UnixStream::connect(socket_path));
    });
    match receiver.recv_timeout(timeout) {
        Ok(Ok(stream)) => Ok(stream),
        Ok(Err(source)) => Err(map_io_error(DaemonClientIoOperationV1::Connect, source)),
        Err(mpsc::RecvTimeoutError::Timeout) => Err(DaemonClientErrorV1::Timeout {
            operation: DaemonClientIoOperationV1::Connect,
        }),
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(DaemonClientErrorV1::Connection {
            operation: DaemonClientIoOperationV1::Connect,
            source: io::Error::other("daemon connection worker exited without a result"),
        }),
    }
}

fn map_frame_error(
    error: FrameErrorV1,
    operation: DaemonClientIoOperationV1,
) -> DaemonClientErrorV1 {
    match error {
        FrameErrorV1::Io { source, .. } => map_io_error(operation, source),
        source => DaemonClientErrorV1::Framing { source },
    }
}

fn map_io_error(operation: DaemonClientIoOperationV1, source: io::Error) -> DaemonClientErrorV1 {
    if source
        .get_ref()
        .is_some_and(|inner| inner.is::<DeadlineSocketConfigurationV1>())
    {
        let source = *source
            .into_inner()
            .expect("deadline configuration error must retain its source")
            .downcast::<DeadlineSocketConfigurationV1>()
            .expect("deadline configuration error type must match");
        return DaemonClientErrorV1::SocketConfiguration {
            operation: source.operation,
            source: source.source,
        };
    }
    if matches!(
        source.kind(),
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock
    ) {
        DaemonClientErrorV1::Timeout { operation }
    } else {
        DaemonClientErrorV1::Connection { operation, source }
    }
}

fn validate_response_correlation(
    request: &RequestEnvelopeV1,
    response: &ResponseEnvelopeV1,
) -> Result<(), DaemonClientErrorV1> {
    let (request_id, command) = match response {
        ResponseEnvelopeV1::Output(output) => (output.request_id(), output.command()),
        ResponseEnvelopeV1::Error(error) => (error.request_id(), error.command()),
    };
    if request_id != request.request_id() {
        return Err(DaemonClientErrorV1::ResponseMismatch {
            field: "request_id",
            expected: request.request_id().as_str().to_owned(),
            received: request_id.as_str().to_owned(),
        });
    }
    if command != request.command() {
        return Err(DaemonClientErrorV1::ResponseMismatch {
            field: "command",
            expected: request.command().as_str().to_owned(),
            received: command.as_str().to_owned(),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        env, fs,
        os::unix::{
            fs::{MetadataExt, PermissionsExt},
            net::UnixListener,
        },
        path::PathBuf,
        sync::atomic::{AtomicU64, Ordering},
    };

    use super::{DaemonClientErrorV1, validate_socket_metadata, validate_socket_parent_metadata};

    static TEST_SEQUENCE: AtomicU64 = AtomicU64::new(0);

    fn temporary_directory() -> PathBuf {
        let sequence = TEST_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let directory = env::temp_dir().join(format!(
            "podway-cli-client-owner-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir(&directory).expect("test directory must be created");
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
            .expect("test directory must be owner-private");
        directory
    }

    #[test]
    fn aut_t_sock_wrong_owner_metadata_fails_closed_without_chown() {
        let directory = temporary_directory();
        let socket_path = directory.join("endpoint.sock");
        let listener = UnixListener::bind(&socket_path).expect("test socket must bind");
        fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600))
            .expect("test socket must be owner-private");

        let parent_metadata = fs::symlink_metadata(&directory).expect("parent metadata");
        let socket_metadata = fs::symlink_metadata(&socket_path).expect("socket metadata");
        let actual_uid = parent_metadata.uid();
        let expected_uid = actual_uid.wrapping_add(1);
        assert_ne!(expected_uid, actual_uid);
        assert_eq!(socket_metadata.uid(), actual_uid);

        let parent_error = validate_socket_parent_metadata(&parent_metadata, expected_uid)
            .expect_err("wrong-owner parent must be rejected");
        assert!(matches!(
            parent_error,
            DaemonClientErrorV1::EndpointSecurity { ref message }
                if message == &format!(
                    "socket parent must be owned by UID {expected_uid} with mode 700"
                )
        ));

        let socket_error = validate_socket_metadata(&socket_metadata, expected_uid)
            .expect_err("wrong-owner socket must be rejected");
        assert!(matches!(
            socket_error,
            DaemonClientErrorV1::EndpointSecurity { ref message }
                if message == &format!(
                    "socket must be a Unix socket owned by UID {expected_uid} with mode 600"
                )
        ));

        drop(listener);
        fs::remove_file(&socket_path).expect("test socket must be removed");
        fs::remove_dir(&directory).expect("test directory must be removed");
    }
}
