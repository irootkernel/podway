//! One-request/one-response Unix-stream transport for the daemon.
//!
//! This module owns connection admission and protocol framing only. Endpoint ownership remains in
//! [`crate::endpoint`], peer authentication remains in [`crate::peer`], and request execution is
//! supplied by an injected dispatcher.

use std::{
    error::Error,
    fmt,
    io::{self, Read, Write},
    num::NonZeroUsize,
    os::unix::net::{UnixListener, UnixStream},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use podway_protocol::{
    CommandNameV1, ErrorCodeV1, ErrorEnvelopeInputV1, ErrorEnvelopeV1, ExitCodeV1,
    FRAME_LENGTH_PREFIX_BYTES_V1, FrameErrorV1, FrameIoPhaseV1, PayloadCodecErrorV1, ProtocolError,
    RequestEnvelopeV1, RequestIdV1, ResponseEnvelopeV1, Rfc3339MillisV1, SUPPORTED_PROTOCOLS_V1,
    SliceRequestV1, decode_request_payload_v1, decode_single_frame_v1, encode_response_payload_v1,
    read_single_frame_v1, validate_frame_payload_length, write_frame_v1,
};
use serde_json::{Map, Value};

use crate::{
    observability::{EventOperationV1, EventOutcomeV1, EventRecordV1, ObservabilityEmitterV1},
    peer::{PeerCredentialSourceV1, PeerUidVerificationErrorV1, PeerUidVerifierV1},
};

/// The documented default deadline for one framed request or response operation.
pub const DEFAULT_FRAME_IO_TIMEOUT_V1: Duration = Duration::from_secs(5);
/// The maximum period a nonblocking accept loop waits before observing an unchanged shutdown state.
pub const DEFAULT_ACCEPT_POLL_INTERVAL_V1: Duration = Duration::from_millis(10);

/// The bounded read and write deadlines applied to every accepted local stream.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ServerTransportTimeoutsV1 {
    read: Duration,
    write: Duration,
}

impl ServerTransportTimeoutsV1 {
    /// Creates nonzero read and write deadlines for accepted local streams.
    pub fn new(
        read: Duration,
        write: Duration,
    ) -> Result<Self, ServerTransportConfigurationErrorV1> {
        if read.is_zero() {
            return Err(ServerTransportConfigurationErrorV1::ZeroReadTimeout);
        }
        if write.is_zero() {
            return Err(ServerTransportConfigurationErrorV1::ZeroWriteTimeout);
        }
        Ok(Self { read, write })
    }

    pub const fn read(self) -> Duration {
        self.read
    }

    pub const fn write(self) -> Duration {
        self.write
    }
}

impl Default for ServerTransportTimeoutsV1 {
    fn default() -> Self {
        Self {
            read: DEFAULT_FRAME_IO_TIMEOUT_V1,
            write: DEFAULT_FRAME_IO_TIMEOUT_V1,
        }
    }
}

/// Configuration failures for the Unix transport and bounded accept loop.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ServerTransportConfigurationErrorV1 {
    ZeroReadTimeout,
    ZeroWriteTimeout,
    ZeroAcceptPollInterval,
}

impl fmt::Display for ServerTransportConfigurationErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroReadTimeout => formatter.write_str("server read timeout must be nonzero"),
            Self::ZeroWriteTimeout => formatter.write_str("server write timeout must be nonzero"),
            Self::ZeroAcceptPollInterval => {
                formatter.write_str("accept-loop poll interval must be nonzero")
            }
        }
    }
}

impl Error for ServerTransportConfigurationErrorV1 {}

/// Failures from the clock used to construct transport response metadata.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResponseMetadataClockErrorV1 {
    BeforeUnixEpoch,
}

impl fmt::Display for ResponseMetadataClockErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BeforeUnixEpoch => formatter.write_str("clock is before the Unix epoch"),
        }
    }
}

impl Error for ResponseMetadataClockErrorV1 {}

/// Supplies a validated elapsed duration for transport response metadata.
pub trait ResponseMetadataClockV1: Send + Sync {
    fn now_since_unix_epoch(&self) -> Result<Duration, ResponseMetadataClockErrorV1>;
}

/// Production wall-clock source for transport response metadata.
#[derive(Clone, Copy, Debug, Default)]
pub struct SystemResponseMetadataClockV1;

impl ResponseMetadataClockV1 for SystemResponseMetadataClockV1 {
    fn now_since_unix_epoch(&self) -> Result<Duration, ResponseMetadataClockErrorV1> {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| ResponseMetadataClockErrorV1::BeforeUnixEpoch)
    }
}

/// Failures that prevent construction of transport-generated response metadata.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResponseMetadataErrorV1 {
    Clock(ResponseMetadataClockErrorV1),
    ClockOutOfRange,
    InvalidTimestamp,
}

impl fmt::Display for ResponseMetadataErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Clock(source) => write!(formatter, "response metadata clock failed: {source}"),
            Self::ClockOutOfRange => {
                formatter.write_str("response metadata clock is outside the supported range")
            }
            Self::InvalidTimestamp => {
                formatter.write_str("response metadata clock produced an invalid timestamp")
            }
        }
    }
}

impl Error for ResponseMetadataErrorV1 {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Clock(source) => Some(source),
            Self::ClockOutOfRange | Self::InvalidTimestamp => None,
        }
    }
}

/// Supplies response metadata for transport-generated protocol errors.
///
/// Dispatchers own metadata for responses to valid requests. The transport only uses this source
/// after a malformed request leaves no valid response metadata to reuse.
pub trait ResponseMetadataSourceV1: Send + Sync {
    fn generated_at(&self) -> Rfc3339MillisV1;
    fn next_request_id(&self) -> RequestIdV1;

    fn try_generated_at(&self) -> Result<Rfc3339MillisV1, ResponseMetadataErrorV1> {
        Ok(self.generated_at())
    }

    fn try_next_request_id(&self) -> Result<RequestIdV1, ResponseMetadataErrorV1> {
        Ok(self.next_request_id())
    }
}

/// Production metadata source for transport-generated errors.
#[derive(Debug)]
pub struct SystemResponseMetadataSourceV1<Clock = SystemResponseMetadataClockV1> {
    sequence: AtomicU64,
    clock: Clock,
}

impl<Clock> SystemResponseMetadataSourceV1<Clock>
where
    Clock: ResponseMetadataClockV1,
{
    /// Constructs the source with an injected wall-clock boundary.
    pub fn with_clock(clock: Clock) -> Self {
        Self {
            sequence: AtomicU64::new(0),
            clock,
        }
    }

    fn clock_milliseconds(&self) -> Result<u64, ResponseMetadataErrorV1> {
        u64::try_from(
            self.clock
                .now_since_unix_epoch()
                .map_err(ResponseMetadataErrorV1::Clock)?
                .as_millis(),
        )
        .map_err(|_| ResponseMetadataErrorV1::ClockOutOfRange)
    }
}

impl Default for SystemResponseMetadataSourceV1<SystemResponseMetadataClockV1> {
    fn default() -> Self {
        Self::with_clock(SystemResponseMetadataClockV1)
    }
}

impl<Clock> ResponseMetadataSourceV1 for SystemResponseMetadataSourceV1<Clock>
where
    Clock: ResponseMetadataClockV1,
{
    fn generated_at(&self) -> Rfc3339MillisV1 {
        self.try_generated_at()
            .expect("system response metadata clock must produce a supported Unix timestamp")
    }

    fn next_request_id(&self) -> RequestIdV1 {
        self.try_next_request_id()
            .expect("system response metadata clock must produce a supported Unix timestamp")
    }

    fn try_generated_at(&self) -> Result<Rfc3339MillisV1, ResponseMetadataErrorV1> {
        let milliseconds = self.clock_milliseconds()?;
        let seconds = milliseconds / 1_000;
        let (year, month, day) = civil_date_from_unix_days((seconds / 86_400) as i64);
        if !(0..=9_999).contains(&year) {
            return Err(ResponseMetadataErrorV1::ClockOutOfRange);
        }
        let second_of_day = seconds % 86_400;
        let hour = second_of_day / 3_600;
        let minute = (second_of_day % 3_600) / 60;
        let second = second_of_day % 60;
        let timestamp = format!(
            "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{:03}Z",
            milliseconds % 1_000
        );
        Rfc3339MillisV1::new(timestamp).map_err(|_| ResponseMetadataErrorV1::InvalidTimestamp)
    }

    fn try_next_request_id(&self) -> Result<RequestIdV1, ResponseMetadataErrorV1> {
        let milliseconds = self.clock_milliseconds()?;
        let sequence = self.sequence.fetch_add(1, Ordering::Relaxed);
        let process_id = u64::from(std::process::id());
        let identifier = format!(
            "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
            (milliseconds >> 32) as u32,
            (milliseconds >> 16) as u16,
            milliseconds as u16,
            sequence as u16,
            (((process_id & 0xffff) << 32) | (sequence & u64::from(u32::MAX)))
        );
        Ok(RequestIdV1::new(identifier).expect(
            "a hexadecimal UUID-shaped identifier constructed from bounded integers is valid",
        ))
    }
}

/// Deterministic transport metadata for tests and embedding environments with their own clock.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FixedResponseMetadataSourceV1 {
    generated_at: Rfc3339MillisV1,
    request_id: RequestIdV1,
}

impl FixedResponseMetadataSourceV1 {
    pub fn new(generated_at: Rfc3339MillisV1, request_id: RequestIdV1) -> Self {
        Self {
            generated_at,
            request_id,
        }
    }
}

impl ResponseMetadataSourceV1 for FixedResponseMetadataSourceV1 {
    fn generated_at(&self) -> Rfc3339MillisV1 {
        self.generated_at.clone()
    }

    fn next_request_id(&self) -> RequestIdV1 {
        self.request_id.clone()
    }
}

/// Dispatches a validated, explicitly admitted G005 request.
///
/// [`SliceRequestV1`] ensures this transport admits only the command set owned by the G005 slice.
/// A dispatcher returns the public success or error envelope for a valid request; transport-level
/// malformed-request errors are constructed before this trait is called.
pub trait RequestDispatcherV1: Send + Sync {
    fn dispatch(
        &self,
        request: &RequestEnvelopeV1,
        slice_request: &SliceRequestV1,
    ) -> ResponseEnvelopeV1;
}

/// Failures that prevent a connection from receiving a complete protocol response.
#[derive(Debug)]
pub enum ServerConnectionErrorV1 {
    Peer(PeerUidVerificationErrorV1),
    ConfigureBlocking(io::Error),
    ConfigureReadTimeout(io::Error),
    ConfigureWriteTimeout(io::Error),
    RequestFrameIo {
        phase: FrameIoPhaseV1,
        kind: io::ErrorKind,
    },
    InvalidDispatcherResponse,
    ResponseMetadata(ResponseMetadataErrorV1),
    ResponseEncode(PayloadCodecErrorV1),
    ResponseWrite(FrameErrorV1),
    ResponseFlush(io::Error),
}

impl fmt::Display for ServerConnectionErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Peer(source) => write!(formatter, "peer admission rejected: {source}"),
            Self::ConfigureBlocking(source) => {
                write!(
                    formatter,
                    "cannot configure accepted server stream as blocking: {source}"
                )
            }
            Self::ConfigureReadTimeout(source) => {
                write!(formatter, "cannot configure server read timeout: {source}")
            }
            Self::ConfigureWriteTimeout(source) => {
                write!(formatter, "cannot configure server write timeout: {source}")
            }
            Self::RequestFrameIo { phase, kind } => {
                write!(
                    formatter,
                    "request frame I/O failed during {phase} ({kind:?})"
                )
            }
            Self::InvalidDispatcherResponse => {
                formatter.write_str("dispatcher emitted an invalid response contract")
            }
            Self::ResponseMetadata(source) => {
                write!(formatter, "cannot construct response metadata: {source}")
            }
            Self::ResponseEncode(source) => {
                write!(formatter, "cannot encode response payload: {source}")
            }
            Self::ResponseWrite(source) => {
                write!(formatter, "cannot write response frame: {source}")
            }
            Self::ResponseFlush(source) => {
                write!(formatter, "cannot flush response frame: {source}")
            }
        }
    }
}

impl Error for ServerConnectionErrorV1 {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Peer(source) => Some(source),
            Self::ConfigureBlocking(source)
            | Self::ConfigureReadTimeout(source)
            | Self::ConfigureWriteTimeout(source)
            | Self::ResponseFlush(source) => Some(source),
            Self::ResponseMetadata(source) => Some(source),
            Self::ResponseEncode(source) => Some(source),
            Self::ResponseWrite(source) => Some(source),
            Self::RequestFrameIo { .. } | Self::InvalidDispatcherResponse => None,
        }
    }
}

/// A one-request/one-response local Unix-stream handler.
pub struct UnixServerTransportV1<Source, Dispatcher, Metadata = SystemResponseMetadataSourceV1> {
    verifier: Arc<PeerUidVerifierV1<Source>>,
    dispatcher: Arc<Dispatcher>,
    metadata: Arc<Metadata>,
    timeouts: ServerTransportTimeoutsV1,
    observability: Option<ObservabilityEmitterV1>,
}

impl<Source, Dispatcher> UnixServerTransportV1<Source, Dispatcher, SystemResponseMetadataSourceV1> {
    pub fn new(
        verifier: PeerUidVerifierV1<Source>,
        dispatcher: Dispatcher,
        timeouts: ServerTransportTimeoutsV1,
    ) -> Self {
        Self::new_with_observability(verifier, dispatcher, timeouts, None)
    }

    pub fn new_with_observability(
        verifier: PeerUidVerifierV1<Source>,
        dispatcher: Dispatcher,
        timeouts: ServerTransportTimeoutsV1,
        observability: Option<ObservabilityEmitterV1>,
    ) -> Self {
        Self::with_metadata_and_observability(
            verifier,
            dispatcher,
            timeouts,
            SystemResponseMetadataSourceV1::default(),
            observability,
        )
    }
}

impl<Source, Dispatcher, Metadata> UnixServerTransportV1<Source, Dispatcher, Metadata> {
    pub fn with_metadata(
        verifier: PeerUidVerifierV1<Source>,
        dispatcher: Dispatcher,
        timeouts: ServerTransportTimeoutsV1,
        metadata: Metadata,
    ) -> Self {
        Self::with_metadata_and_observability(verifier, dispatcher, timeouts, metadata, None)
    }

    pub fn with_metadata_and_observability(
        verifier: PeerUidVerifierV1<Source>,
        dispatcher: Dispatcher,
        timeouts: ServerTransportTimeoutsV1,
        metadata: Metadata,
        observability: Option<ObservabilityEmitterV1>,
    ) -> Self {
        Self {
            verifier: Arc::new(verifier),
            dispatcher: Arc::new(dispatcher),
            metadata: Arc::new(metadata),
            timeouts,
            observability,
        }
    }

    pub fn verifier(&self) -> &PeerUidVerifierV1<Source> {
        &self.verifier
    }

    pub fn dispatcher(&self) -> &Dispatcher {
        &self.dispatcher
    }

    pub fn timeouts(&self) -> ServerTransportTimeoutsV1 {
        self.timeouts
    }
}

trait ConnectionSetupV1 {
    fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()>;
    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()>;
    fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()>;
}

impl ConnectionSetupV1 for UnixStream {
    fn set_nonblocking(&self, nonblocking: bool) -> io::Result<()> {
        UnixStream::set_nonblocking(self, nonblocking)
    }

    fn set_read_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        UnixStream::set_read_timeout(self, timeout)
    }

    fn set_write_timeout(&self, timeout: Option<Duration>) -> io::Result<()> {
        UnixStream::set_write_timeout(self, timeout)
    }
}

fn configure_connection(
    connection: &impl ConnectionSetupV1,
    timeouts: ServerTransportTimeoutsV1,
) -> Result<(), ServerConnectionErrorV1> {
    connection
        .set_nonblocking(false)
        .map_err(ServerConnectionErrorV1::ConfigureBlocking)?;
    connection
        .set_read_timeout(Some(timeouts.read()))
        .map_err(ServerConnectionErrorV1::ConfigureReadTimeout)?;
    connection
        .set_write_timeout(Some(timeouts.write()))
        .map_err(ServerConnectionErrorV1::ConfigureWriteTimeout)
}

impl<Source, Dispatcher, Metadata> UnixServerTransportV1<Source, Dispatcher, Metadata>
where
    Source: PeerCredentialSourceV1,
    Dispatcher: RequestDispatcherV1,
    Metadata: ResponseMetadataSourceV1,
{
    /// Verifies the Unix peer before consuming frame bytes, then processes exactly one request.
    pub fn handle_connection(
        &self,
        mut connection: UnixStream,
    ) -> Result<(), ServerConnectionErrorV1> {
        if let Err(error) = self.verifier.verify(&connection) {
            emit_observation(
                &self.observability,
                EventOperationV1::PeerAdmission,
                EventOutcomeV1::Rejected,
            );
            return Err(ServerConnectionErrorV1::Peer(error));
        }
        if let Err(error) = configure_connection(&connection, self.timeouts) {
            emit_observation(
                &self.observability,
                EventOperationV1::ConnectionSetup,
                EventOutcomeV1::Failed,
            );
            return Err(error);
        }

        let (frame, recorded) = {
            let mut recorder = RecordingReaderV1::new(&mut connection);
            let frame = read_single_frame_v1(&mut recorder);
            (frame, recorder.into_bytes())
        };
        let payload = match frame {
            Ok(Some(payload)) => payload,
            Ok(None) => {
                emit_observation(
                    &self.observability,
                    EventOperationV1::TransportServiceRequest,
                    EventOutcomeV1::Rejected,
                );
                return self.write_transport_error(
                    &mut connection,
                    None,
                    TransportErrorKindV1::InvalidRequest,
                );
            }
            Err(error) => {
                emit_observation(
                    &self.observability,
                    EventOperationV1::TransportServiceRequest,
                    EventOutcomeV1::Rejected,
                );
                let context = recover_request_context_from_recorded_frame(&recorded);
                self.write_transport_error(&mut connection, context, classify_frame_error(&error))?;
                return match error {
                    FrameErrorV1::Io { phase, source } => {
                        Err(ServerConnectionErrorV1::RequestFrameIo {
                            phase,
                            kind: source.kind(),
                        })
                    }
                    FrameErrorV1::InvalidLength(_)
                    | FrameErrorV1::UnexpectedEof { .. }
                    | FrameErrorV1::TrailingData => Ok(()),
                };
            }
        };

        let request = match decode_request_payload_v1(&payload) {
            Ok(request) => request,
            Err(error) => {
                emit_observation(
                    &self.observability,
                    EventOperationV1::TransportServiceRequest,
                    EventOutcomeV1::Rejected,
                );
                return self.write_transport_error(
                    &mut connection,
                    recover_request_context(&payload),
                    classify_payload_error(&error),
                );
            }
        };
        let slice_request = match SliceRequestV1::from_envelope(&request) {
            Ok(slice_request) => slice_request,
            Err(_) => {
                emit_observation(
                    &self.observability,
                    EventOperationV1::TransportServiceRequest,
                    EventOutcomeV1::Rejected,
                );
                return self.write_transport_error(
                    &mut connection,
                    Some(RequestContextV1::from_request(&request)),
                    TransportErrorKindV1::InvalidRequest,
                );
            }
        };

        let response = self.dispatcher.dispatch(&request, &slice_request);
        if response_matches_request(&response, &request) && response.validate().is_ok() {
            emit_observation(
                &self.observability,
                EventOperationV1::ServiceDispatch,
                match &response {
                    ResponseEnvelopeV1::Output(_) => EventOutcomeV1::Succeeded,
                    ResponseEnvelopeV1::Error(_) => EventOutcomeV1::Rejected,
                },
            );
            return self.write_response(&mut connection, &response);
        }

        emit_observation(
            &self.observability,
            EventOperationV1::ServiceDispatch,
            EventOutcomeV1::Failed,
        );
        let response = self.transport_error_response(
            Some(RequestContextV1::from_request(&request)),
            TransportErrorKindV1::Internal,
        )?;
        self.write_response(&mut connection, &response)?;
        Err(ServerConnectionErrorV1::InvalidDispatcherResponse)
    }

    fn write_transport_error(
        &self,
        connection: &mut UnixStream,
        context: Option<RequestContextV1>,
        kind: TransportErrorKindV1,
    ) -> Result<(), ServerConnectionErrorV1> {
        let response = self.transport_error_response(context, kind)?;
        self.write_response(connection, &response)
    }

    fn transport_error_response(
        &self,
        context: Option<RequestContextV1>,
        kind: TransportErrorKindV1,
    ) -> Result<ResponseEnvelopeV1, ServerConnectionErrorV1> {
        let (request_id, command, request_id_recovered) = match context {
            Some(context) => (context.request_id, context.command, true),
            None => (
                self.metadata
                    .try_next_request_id()
                    .map_err(ServerConnectionErrorV1::ResponseMetadata)?,
                fallback_command(),
                false,
            ),
        };
        let mut details = Map::new();
        if !request_id_recovered {
            details.insert("request_id_recovered".to_owned(), Value::Bool(false));
        }
        if kind == TransportErrorKindV1::UnsupportedProtocol {
            details.insert(
                "supported_protocols".to_owned(),
                Value::Array(
                    SUPPORTED_PROTOCOLS_V1
                        .iter()
                        .map(|protocol| Value::String((*protocol).to_owned()))
                        .collect(),
                ),
            );
        }

        let (code, message, exit_code) = match kind {
            TransportErrorKindV1::InvalidRequest => (
                "REQUEST_INVALID",
                "Request is malformed or violates schema.",
                2,
            ),
            TransportErrorKindV1::RequestTooLarge => (
                "REQUEST_TOO_LARGE",
                "IPC request exceeds configured limits.",
                2,
            ),
            TransportErrorKindV1::UnsupportedProtocol => (
                "PROTOCOL_VERSION_UNSUPPORTED",
                "Requested IPC protocol is unsupported.",
                3,
            ),
            TransportErrorKindV1::Internal => (
                "INTERNAL_ERROR",
                "An unexpected internal error occurred.",
                6,
            ),
        };
        Ok(ResponseEnvelopeV1::Error(
            ErrorEnvelopeV1::new(ErrorEnvelopeInputV1 {
                request_id,
                command,
                generated_at: self
                    .metadata
                    .try_generated_at()
                    .map_err(ServerConnectionErrorV1::ResponseMetadata)?,
                code: ErrorCodeV1::new(code)
                    .expect("the transport only uses catalog-defined error codes"),
                message: message.to_owned(),
                retryable: false,
                exit_code: ExitCodeV1::new(exit_code)
                    .expect("the transport only uses catalog-defined exit codes"),
                workspace: None,
                details,
            })
            .expect("the transport constructs a protocol-valid error envelope"),
        ))
    }

    fn write_response(
        &self,
        connection: &mut UnixStream,
        response: &ResponseEnvelopeV1,
    ) -> Result<(), ServerConnectionErrorV1> {
        let result = (|| {
            let payload = encode_response_payload_v1(response)
                .map_err(ServerConnectionErrorV1::ResponseEncode)?;
            write_frame_v1(connection, &payload).map_err(ServerConnectionErrorV1::ResponseWrite)?;
            connection
                .flush()
                .map_err(ServerConnectionErrorV1::ResponseFlush)
        })();
        if result.is_err() {
            emit_observation(
                &self.observability,
                EventOperationV1::ResponseWrite,
                EventOutcomeV1::Failed,
            );
        }
        result
    }
}

#[derive(Clone, Debug)]
struct RequestContextV1 {
    request_id: RequestIdV1,
    command: CommandNameV1,
}

impl RequestContextV1 {
    fn from_request(request: &RequestEnvelopeV1) -> Self {
        Self {
            request_id: request.request_id().clone(),
            command: request.command().clone(),
        }
    }
}

struct RecordingReaderV1<'a> {
    inner: &'a mut UnixStream,
    bytes: Vec<u8>,
}

impl<'a> RecordingReaderV1<'a> {
    fn new(inner: &'a mut UnixStream) -> Self {
        Self {
            inner,
            bytes: Vec::new(),
        }
    }

    fn into_bytes(self) -> Vec<u8> {
        self.bytes
    }
}

impl Read for RecordingReaderV1<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let received = self.inner.read(buffer)?;
        self.bytes.extend_from_slice(&buffer[..received]);
        Ok(received)
    }
}

fn recover_request_context_from_recorded_frame(recorded: &[u8]) -> Option<RequestContextV1> {
    if recorded.len() < FRAME_LENGTH_PREFIX_BYTES_V1 {
        return None;
    }
    let mut prefix = [0_u8; FRAME_LENGTH_PREFIX_BYTES_V1];
    prefix.copy_from_slice(&recorded[..FRAME_LENGTH_PREFIX_BYTES_V1]);
    let length = usize::try_from(u32::from_be_bytes(prefix)).ok()?;
    validate_frame_payload_length(length).ok()?;
    let frame_end = FRAME_LENGTH_PREFIX_BYTES_V1.checked_add(length)?;
    let frame = recorded.get(..frame_end)?;
    recover_request_context(decode_single_frame_v1(frame).ok()?)
}
/// Recovers a valid request ID independently of the rest of a malformed JSON request.
fn recover_request_context(payload: &[u8]) -> Option<RequestContextV1> {
    let document = std::str::from_utf8(payload).ok()?;
    let document = serde_json::from_str::<Value>(document).ok()?;
    let object = document.as_object()?;
    let request_id = RequestIdV1::new(object.get("request_id")?.as_str()?).ok()?;
    let command = object
        .get("command")
        .and_then(Value::as_str)
        .and_then(|command| CommandNameV1::new(command).ok())
        .unwrap_or_else(fallback_command);
    Some(RequestContextV1 {
        request_id,
        command,
    })
}

fn fallback_command() -> CommandNameV1 {
    CommandNameV1::new("ipc.request")
        .expect("the fixed transport fallback command is protocol-valid")
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TransportErrorKindV1 {
    InvalidRequest,
    RequestTooLarge,
    UnsupportedProtocol,
    Internal,
}

fn classify_frame_error(error: &FrameErrorV1) -> TransportErrorKindV1 {
    match error {
        FrameErrorV1::InvalidLength(ProtocolError::FrameTooLarge { .. }) => {
            TransportErrorKindV1::RequestTooLarge
        }
        FrameErrorV1::InvalidLength(_)
        | FrameErrorV1::UnexpectedEof { .. }
        | FrameErrorV1::TrailingData => TransportErrorKindV1::InvalidRequest,
        FrameErrorV1::Io { .. } => TransportErrorKindV1::Internal,
    }
}

fn classify_payload_error(error: &PayloadCodecErrorV1) -> TransportErrorKindV1 {
    match error {
        PayloadCodecErrorV1::JsonContract(ProtocolError::UnsupportedProtocol { .. }) => {
            TransportErrorKindV1::UnsupportedProtocol
        }
        _ => TransportErrorKindV1::InvalidRequest,
    }
}

fn response_matches_request(response: &ResponseEnvelopeV1, request: &RequestEnvelopeV1) -> bool {
    match response {
        ResponseEnvelopeV1::Output(output) => {
            output.request_id() == request.request_id() && output.command() == request.command()
        }
        ResponseEnvelopeV1::Error(error) => {
            error.request_id() == request.request_id() && error.command() == request.command()
        }
    }
}

fn emit_observation(
    observability: &Option<ObservabilityEmitterV1>,
    operation: EventOperationV1,
    outcome: EventOutcomeV1,
) {
    if let Some(observability) = observability {
        observability.emit(EventRecordV1::new(operation, outcome));
    }
}
/// Failures that invalidate the admission invariant.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShutdownAdmissionErrorV1 {
    PoisonedState,
}

impl fmt::Display for ShutdownAdmissionErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PoisonedState => formatter.write_str("shutdown admission state is poisoned"),
        }
    }
}

impl Error for ShutdownAdmissionErrorV1 {}

/// Explicit shared state for admission and draining of an accept loop.
#[derive(Clone, Debug)]
pub struct ShutdownAdmissionV1 {
    inner: Arc<ShutdownAdmissionInnerV1>,
}

#[derive(Debug)]
struct ShutdownAdmissionInnerV1 {
    state: Mutex<ShutdownAdmissionStateV1>,
    changed: Condvar,
}

#[derive(Debug)]
struct ShutdownAdmissionStateV1 {
    accepting: bool,
    in_flight: usize,
}

#[derive(Debug)]
enum ShutdownAdmissionOutcomeV1 {
    Admitted(ShutdownAdmissionTicketV1),
    Closed,
    AtCapacity,
}

impl ShutdownAdmissionV1 {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(ShutdownAdmissionInnerV1 {
                state: Mutex::new(ShutdownAdmissionStateV1 {
                    accepting: true,
                    in_flight: 0,
                }),
                changed: Condvar::new(),
            }),
        }
    }

    /// Stops future admission. Existing tickets remain live until their handlers return.
    pub fn request_shutdown(&self) {
        let mut state = match self.inner.state.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        };
        state.accepting = false;
        self.inner.changed.notify_all();
    }

    pub fn is_accepting(&self) -> bool {
        self.try_is_accepting().unwrap_or(false)
    }

    pub fn in_flight(&self) -> usize {
        self.try_in_flight().unwrap_or_default()
    }

    fn lock_state(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, ShutdownAdmissionStateV1>, ShutdownAdmissionErrorV1> {
        match self.inner.state.lock() {
            Ok(state) => Ok(state),
            Err(poisoned) => {
                let mut state = poisoned.into_inner();
                state.accepting = false;
                self.inner.changed.notify_all();
                Err(ShutdownAdmissionErrorV1::PoisonedState)
            }
        }
    }

    fn try_is_accepting(&self) -> Result<bool, ShutdownAdmissionErrorV1> {
        Ok(self.lock_state()?.accepting)
    }

    fn try_in_flight(&self) -> Result<usize, ShutdownAdmissionErrorV1> {
        Ok(self.lock_state()?.in_flight)
    }

    fn try_admit(
        &self,
        maximum: NonZeroUsize,
    ) -> Result<ShutdownAdmissionOutcomeV1, ShutdownAdmissionErrorV1> {
        let mut state = self.lock_state()?;
        if !state.accepting {
            return Ok(ShutdownAdmissionOutcomeV1::Closed);
        }
        if state.in_flight >= maximum.get() {
            return Ok(ShutdownAdmissionOutcomeV1::AtCapacity);
        }
        state.in_flight += 1;
        Ok(ShutdownAdmissionOutcomeV1::Admitted(
            ShutdownAdmissionTicketV1 {
                admission: self.clone(),
            },
        ))
    }

    fn at_capacity(&self, maximum: NonZeroUsize) -> Result<bool, ShutdownAdmissionErrorV1> {
        Ok(self.lock_state()?.in_flight >= maximum.get())
    }

    fn wait_for_progress(&self, timeout: Duration) -> Result<(), ShutdownAdmissionErrorV1> {
        let state = self.lock_state()?;
        match self.inner.changed.wait_timeout(state, timeout) {
            Ok((_state, _)) => Ok(()),
            Err(poisoned) => {
                let (mut state, _) = poisoned.into_inner();
                state.accepting = false;
                self.inner.changed.notify_all();
                Err(ShutdownAdmissionErrorV1::PoisonedState)
            }
        }
    }
}

impl Default for ShutdownAdmissionV1 {
    fn default() -> Self {
        Self::new()
    }
}

/// The RAII token that proves a handler was admitted before shutdown began.
#[derive(Debug)]
struct ShutdownAdmissionTicketV1 {
    admission: ShutdownAdmissionV1,
}

impl Drop for ShutdownAdmissionTicketV1 {
    fn drop(&mut self) {
        let mut state = match self.admission.inner.state.lock() {
            Ok(state) => state,
            Err(poisoned) => {
                let mut state = poisoned.into_inner();
                state.accepting = false;
                state
            }
        };
        state.in_flight = state
            .in_flight
            .checked_sub(1)
            .expect("every admission ticket decrements exactly one admitted handler");
        self.admission.inner.changed.notify_all();
    }
}

/// Spawns an admitted connection handler at the accept-loop boundary.
pub trait ConnectionHandlerSpawnerV1: Send + Sync {
    fn spawn(&self, handler: Box<dyn FnOnce() + Send + 'static>) -> io::Result<JoinHandle<()>>;
}

/// Production connection-handler spawner.
#[derive(Debug, Default)]
pub struct ThreadConnectionHandlerSpawnerV1;

impl ConnectionHandlerSpawnerV1 for ThreadConnectionHandlerSpawnerV1 {
    fn spawn(&self, handler: Box<dyn FnOnce() + Send + 'static>) -> io::Result<JoinHandle<()>> {
        thread::Builder::new()
            .name("podwayd-connection".to_owned())
            .spawn(handler)
    }
}

/// A bounded nonblocking accept-loop primitive with explicit admission and drain state.
pub struct BoundedAcceptLoopV1<Source, Dispatcher, Metadata = SystemResponseMetadataSourceV1> {
    transport: Arc<UnixServerTransportV1<Source, Dispatcher, Metadata>>,
    admission: ShutdownAdmissionV1,
    maximum_in_flight: NonZeroUsize,
    poll_interval: Duration,
    handler_spawner: Arc<dyn ConnectionHandlerSpawnerV1>,
    observability: Option<ObservabilityEmitterV1>,
}

impl<Source, Dispatcher, Metadata> BoundedAcceptLoopV1<Source, Dispatcher, Metadata> {
    pub fn new(
        transport: Arc<UnixServerTransportV1<Source, Dispatcher, Metadata>>,
        admission: ShutdownAdmissionV1,
        maximum_in_flight: NonZeroUsize,
    ) -> Self {
        Self::new_with_observability(transport, admission, maximum_in_flight, None)
    }

    pub fn new_with_observability(
        transport: Arc<UnixServerTransportV1<Source, Dispatcher, Metadata>>,
        admission: ShutdownAdmissionV1,
        maximum_in_flight: NonZeroUsize,
        observability: Option<ObservabilityEmitterV1>,
    ) -> Self {
        Self {
            transport,
            admission,
            maximum_in_flight,
            poll_interval: DEFAULT_ACCEPT_POLL_INTERVAL_V1,
            handler_spawner: Arc::new(ThreadConnectionHandlerSpawnerV1),
            observability,
        }
    }

    pub fn with_poll_interval(
        transport: Arc<UnixServerTransportV1<Source, Dispatcher, Metadata>>,
        admission: ShutdownAdmissionV1,
        maximum_in_flight: NonZeroUsize,
        poll_interval: Duration,
    ) -> Result<Self, ServerTransportConfigurationErrorV1> {
        Self::with_poll_interval_and_handler_spawner(
            transport,
            admission,
            maximum_in_flight,
            poll_interval,
            Arc::new(ThreadConnectionHandlerSpawnerV1),
        )
    }

    /// Constructs an accept loop with an injected connection-handler spawn boundary.
    pub fn with_poll_interval_and_handler_spawner(
        transport: Arc<UnixServerTransportV1<Source, Dispatcher, Metadata>>,
        admission: ShutdownAdmissionV1,
        maximum_in_flight: NonZeroUsize,
        poll_interval: Duration,
        handler_spawner: Arc<dyn ConnectionHandlerSpawnerV1>,
    ) -> Result<Self, ServerTransportConfigurationErrorV1> {
        Self::with_poll_interval_handler_spawner_and_observability(
            transport,
            admission,
            maximum_in_flight,
            poll_interval,
            handler_spawner,
            None,
        )
    }

    /// Constructs an accept loop with deterministic handler and observation boundaries.
    pub fn with_poll_interval_handler_spawner_and_observability(
        transport: Arc<UnixServerTransportV1<Source, Dispatcher, Metadata>>,
        admission: ShutdownAdmissionV1,
        maximum_in_flight: NonZeroUsize,
        poll_interval: Duration,
        handler_spawner: Arc<dyn ConnectionHandlerSpawnerV1>,
        observability: Option<ObservabilityEmitterV1>,
    ) -> Result<Self, ServerTransportConfigurationErrorV1> {
        if poll_interval.is_zero() {
            return Err(ServerTransportConfigurationErrorV1::ZeroAcceptPollInterval);
        }
        Ok(Self {
            transport,
            admission,
            maximum_in_flight,
            poll_interval,
            handler_spawner,
            observability,
        })
    }

    pub fn admission(&self) -> &ShutdownAdmissionV1 {
        &self.admission
    }
}

impl<Source, Dispatcher, Metadata> BoundedAcceptLoopV1<Source, Dispatcher, Metadata>
where
    Source: PeerCredentialSourceV1 + Send + Sync + 'static,
    Dispatcher: RequestDispatcherV1 + 'static,
    Metadata: ResponseMetadataSourceV1 + 'static,
{
    /// Accepts only while admission is open, then joins every already-admitted handler before return.
    ///
    /// The loop intentionally owns no endpoint path state. Callers pass the listener supplied by
    /// [`crate::endpoint::SingletonEndpointGuardV1`].
    pub fn run(&self, listener: &UnixListener) -> Result<(), ServerAcceptLoopErrorV1> {
        if let Err(error) = listener.set_nonblocking(true) {
            emit_observation(
                &self.observability,
                EventOperationV1::ConnectionSetup,
                EventOutcomeV1::Failed,
            );
            return Err(ServerAcceptLoopErrorV1::ConfigureNonblocking(error));
        }

        let mut handlers = Vec::new();
        let mut terminal_error = None;
        let mut saturation_reported = false;
        loop {
            match self.admission.try_is_accepting() {
                Ok(true) => {}
                Ok(false) => break,
                Err(error) => {
                    emit_observation(
                        &self.observability,
                        EventOperationV1::ConnectionSetup,
                        EventOutcomeV1::Failed,
                    );
                    terminal_error = Some(ServerAcceptLoopErrorV1::Admission(error));
                    break;
                }
            }
            if reap_finished_handlers(&mut handlers) {
                emit_observation(
                    &self.observability,
                    EventOperationV1::HandlerJoin,
                    EventOutcomeV1::Failed,
                );
                self.admission.request_shutdown();
                terminal_error = Some(ServerAcceptLoopErrorV1::HandlerPanicked);
                continue;
            }
            match self.admission.at_capacity(self.maximum_in_flight) {
                Ok(true) => {}
                Ok(false) => saturation_reported = false,
                Err(error) => {
                    emit_observation(
                        &self.observability,
                        EventOperationV1::ConnectionSetup,
                        EventOutcomeV1::Failed,
                    );
                    terminal_error = Some(ServerAcceptLoopErrorV1::Admission(error));
                    break;
                }
            }

            match listener.accept() {
                Ok((connection, _)) => {
                    emit_observation(
                        &self.observability,
                        EventOperationV1::ConnectionAccepted,
                        EventOutcomeV1::Succeeded,
                    );
                    let ticket = match self.admission.try_admit(self.maximum_in_flight) {
                        Ok(ShutdownAdmissionOutcomeV1::Admitted(ticket)) => ticket,
                        Ok(ShutdownAdmissionOutcomeV1::Closed) => continue,
                        Ok(ShutdownAdmissionOutcomeV1::AtCapacity) => {
                            if !saturation_reported {
                                emit_observation(
                                    &self.observability,
                                    EventOperationV1::AdmissionSaturation,
                                    EventOutcomeV1::Saturated,
                                );
                                saturation_reported = true;
                            }
                            continue;
                        }
                        Err(error) => {
                            emit_observation(
                                &self.observability,
                                EventOperationV1::ConnectionSetup,
                                EventOutcomeV1::Failed,
                            );
                            terminal_error = Some(ServerAcceptLoopErrorV1::Admission(error));
                            break;
                        }
                    };
                    let transport = Arc::clone(&self.transport);
                    let handler = Box::new(move || {
                        let _ticket = ticket;
                        let _ = transport.handle_connection(connection);
                    });
                    match self.handler_spawner.spawn(handler) {
                        Ok(handler) => handlers.push(handler),
                        Err(error) => {
                            emit_observation(
                                &self.observability,
                                EventOperationV1::ConnectionSetup,
                                EventOutcomeV1::Failed,
                            );
                            self.admission.request_shutdown();
                            terminal_error = Some(ServerAcceptLoopErrorV1::SpawnHandler(error));
                            break;
                        }
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                    if let Err(error) = self.admission.wait_for_progress(self.poll_interval) {
                        emit_observation(
                            &self.observability,
                            EventOperationV1::ConnectionSetup,
                            EventOutcomeV1::Failed,
                        );
                        terminal_error = Some(ServerAcceptLoopErrorV1::Admission(error));
                        break;
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => {
                    emit_observation(
                        &self.observability,
                        EventOperationV1::ConnectionSetup,
                        EventOutcomeV1::Failed,
                    );
                    self.admission.request_shutdown();
                    terminal_error = Some(ServerAcceptLoopErrorV1::Accept(error));
                    break;
                }
            }
        }

        if reap_all_handlers(handlers) {
            emit_observation(
                &self.observability,
                EventOperationV1::HandlerJoin,
                EventOutcomeV1::Failed,
            );
            terminal_error.get_or_insert(ServerAcceptLoopErrorV1::HandlerPanicked);
        }
        match terminal_error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

/// Failures of the accept loop itself. Per-connection failures close only that connection.
#[derive(Debug)]
pub enum ServerAcceptLoopErrorV1 {
    ConfigureNonblocking(io::Error),
    Accept(io::Error),
    SpawnHandler(io::Error),
    Admission(ShutdownAdmissionErrorV1),
    HandlerPanicked,
}

impl fmt::Display for ServerAcceptLoopErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ConfigureNonblocking(source) => {
                write!(
                    formatter,
                    "cannot configure listener as nonblocking: {source}"
                )
            }
            Self::Accept(source) => write!(formatter, "cannot accept Unix connection: {source}"),
            Self::SpawnHandler(source) => {
                write!(formatter, "cannot spawn admitted server handler: {source}")
            }
            Self::Admission(source) => write!(formatter, "server admission failed: {source}"),
            Self::HandlerPanicked => formatter.write_str("an admitted server handler panicked"),
        }
    }
}

impl Error for ServerAcceptLoopErrorV1 {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::ConfigureNonblocking(source)
            | Self::Accept(source)
            | Self::SpawnHandler(source) => Some(source),
            Self::Admission(source) => Some(source),
            Self::HandlerPanicked => None,
        }
    }
}

fn reap_finished_handlers(handlers: &mut Vec<JoinHandle<()>>) -> bool {
    let mut remaining = Vec::with_capacity(handlers.len());
    let mut panicked = false;
    for handler in handlers.drain(..) {
        if handler.is_finished() {
            panicked |= handler.join().is_err();
        } else {
            remaining.push(handler);
        }
    }
    *handlers = remaining;
    panicked
}

fn reap_all_handlers(handlers: Vec<JoinHandle<()>>) -> bool {
    let mut panicked = false;
    for handler in handlers {
        panicked |= handler.join().is_err();
    }
    panicked
}

fn civil_date_from_unix_days(days: i64) -> (i64, u32, u32) {
    let days = days + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let day_of_era = days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_parameter = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_parameter + 2) / 5 + 1;
    let month = month_parameter + if month_parameter < 10 { 3 } else { -9 };
    (year + i64::from(month <= 2), month as u32, day as u32)
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admission_outcomes_distinguish_capacity_from_shutdown_closure() {
        let admission = ShutdownAdmissionV1::new();
        let maximum = NonZeroUsize::new(1).expect("one is nonzero");
        let ticket = match admission
            .try_admit(maximum)
            .expect("fresh admission state must not be poisoned")
        {
            ShutdownAdmissionOutcomeV1::Admitted(ticket) => ticket,
            ShutdownAdmissionOutcomeV1::Closed | ShutdownAdmissionOutcomeV1::AtCapacity => {
                panic!("fresh admission state must admit one ticket")
            }
        };

        assert!(matches!(
            admission.try_admit(maximum),
            Ok(ShutdownAdmissionOutcomeV1::AtCapacity)
        ));
        drop(ticket);
        admission.request_shutdown();
        assert!(matches!(
            admission.try_admit(maximum),
            Ok(ShutdownAdmissionOutcomeV1::Closed)
        ));
    }
    #[test]
    fn poisoned_admission_closes_and_releases_existing_tickets() {
        let admission = ShutdownAdmissionV1::new();
        let ticket = match admission
            .try_admit(NonZeroUsize::new(1).expect("one is nonzero"))
            .expect("fresh admission state must not be poisoned")
        {
            ShutdownAdmissionOutcomeV1::Admitted(ticket) => ticket,
            ShutdownAdmissionOutcomeV1::Closed | ShutdownAdmissionOutcomeV1::AtCapacity => {
                panic!("fresh admission state must admit one ticket")
            }
        };
        let inner = Arc::clone(&admission.inner);
        let poisoner = thread::spawn(move || {
            let _guard = inner.state.lock().expect("fresh state lock must succeed");
            panic!("inject shutdown admission state poison");
        });
        assert!(poisoner.join().is_err());

        assert!(matches!(
            admission.try_is_accepting(),
            Err(ShutdownAdmissionErrorV1::PoisonedState)
        ));
        assert!(!admission.is_accepting());

        drop(ticket);

        let state = admission
            .inner
            .state
            .lock()
            .expect_err("injected state lock must remain poisoned")
            .into_inner();
        assert!(!state.accepting);
        assert_eq!(state.in_flight, 0);
    }
    #[derive(Clone, Copy)]
    enum SetupFailureV1 {
        Blocking,
        ReadTimeout,
        WriteTimeout,
    }

    struct FailingConnectionSetupV1 {
        failure: SetupFailureV1,
        calls: std::cell::Cell<usize>,
    }

    impl ConnectionSetupV1 for FailingConnectionSetupV1 {
        fn set_nonblocking(&self, _: bool) -> io::Result<()> {
            self.calls.set(self.calls.get() + 1);
            match self.failure {
                SetupFailureV1::Blocking => Err(io::Error::other("injected blocking failure")),
                SetupFailureV1::ReadTimeout | SetupFailureV1::WriteTimeout => Ok(()),
            }
        }

        fn set_read_timeout(&self, _: Option<Duration>) -> io::Result<()> {
            self.calls.set(self.calls.get() + 1);
            match self.failure {
                SetupFailureV1::ReadTimeout => {
                    Err(io::Error::other("injected read timeout failure"))
                }
                SetupFailureV1::Blocking | SetupFailureV1::WriteTimeout => Ok(()),
            }
        }

        fn set_write_timeout(&self, _: Option<Duration>) -> io::Result<()> {
            self.calls.set(self.calls.get() + 1);
            match self.failure {
                SetupFailureV1::WriteTimeout => {
                    Err(io::Error::other("injected write timeout failure"))
                }
                SetupFailureV1::Blocking | SetupFailureV1::ReadTimeout => Ok(()),
            }
        }
    }

    #[test]
    fn connection_setup_failures_are_typed_and_stop_at_the_failing_step() {
        for (failure, expected_calls) in [
            (SetupFailureV1::Blocking, 1),
            (SetupFailureV1::ReadTimeout, 2),
            (SetupFailureV1::WriteTimeout, 3),
        ] {
            let connection = FailingConnectionSetupV1 {
                failure,
                calls: std::cell::Cell::new(0),
            };
            let error = configure_connection(&connection, ServerTransportTimeoutsV1::default())
                .expect_err("injected setup operation must fail");

            assert_eq!(connection.calls.get(), expected_calls);
            match failure {
                SetupFailureV1::Blocking => {
                    assert!(matches!(
                        error,
                        ServerConnectionErrorV1::ConfigureBlocking(_)
                    ));
                }
                SetupFailureV1::ReadTimeout => {
                    assert!(matches!(
                        error,
                        ServerConnectionErrorV1::ConfigureReadTimeout(_)
                    ));
                }
                SetupFailureV1::WriteTimeout => {
                    assert!(matches!(
                        error,
                        ServerConnectionErrorV1::ConfigureWriteTimeout(_)
                    ));
                }
            }
        }
    }
}
