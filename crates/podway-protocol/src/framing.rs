use std::{
    fmt,
    io::{self, Read, Write},
};

use crate::{FRAME_LENGTH_PREFIX_BYTES_V1, ProtocolError, validate_frame_payload_length};

/// The stream phase in which a frame I/O error occurred.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameIoPhaseV1 {
    LengthPrefix,
    Payload,
    EndOfStream,
}

impl fmt::Display for FrameIoPhaseV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LengthPrefix => formatter.write_str("length prefix"),
            Self::Payload => formatter.write_str("payload"),
            Self::EndOfStream => formatter.write_str("end of stream"),
        }
    }
}

/// A framing failure that preserves validation, EOF, and stream-I/O distinctions.
#[derive(Debug)]
pub enum FrameErrorV1 {
    InvalidLength(ProtocolError),
    UnexpectedEof {
        phase: FrameIoPhaseV1,
        expected: usize,
        received: usize,
    },
    TrailingData,
    Io {
        phase: FrameIoPhaseV1,
        source: io::Error,
    },
}

impl FrameErrorV1 {
    /// Returns the I/O phase for EOF and I/O failures.
    pub const fn phase(&self) -> Option<FrameIoPhaseV1> {
        match self {
            Self::UnexpectedEof { phase, .. } | Self::Io { phase, .. } => Some(*phase),
            Self::InvalidLength(_) | Self::TrailingData => None,
        }
    }
}

impl fmt::Display for FrameErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidLength(error) => error.fmt(formatter),
            Self::UnexpectedEof {
                phase,
                expected,
                received,
            } => write!(
                formatter,
                "unexpected EOF while reading {phase}: expected {expected} bytes, received {received}"
            ),
            Self::TrailingData => formatter.write_str("trailing data after a single frame"),
            Self::Io { phase, source } => write!(formatter, "I/O failure during {phase}: {source}"),
        }
    }
}

impl std::error::Error for FrameErrorV1 {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidLength(error) => Some(error),
            Self::Io { source, .. } => Some(source),
            Self::UnexpectedEof { .. } | Self::TrailingData => None,
        }
    }
}

/// Encodes one bounded v1 frame as its four-byte unsigned big-endian length prefix and payload.
pub fn encode_frame_v1(payload: &[u8]) -> Result<Vec<u8>, ProtocolError> {
    let prefix = frame_prefix(payload.len())?;
    let mut frame = Vec::with_capacity(FRAME_LENGTH_PREFIX_BYTES_V1 + payload.len());
    frame.extend_from_slice(&prefix);
    frame.extend_from_slice(payload);
    Ok(frame)
}

/// Decodes exactly one frame from a complete byte slice without allocating.
pub fn decode_single_frame_v1(input: &[u8]) -> Result<&[u8], FrameErrorV1> {
    if input.len() < FRAME_LENGTH_PREFIX_BYTES_V1 {
        return Err(FrameErrorV1::UnexpectedEof {
            phase: FrameIoPhaseV1::LengthPrefix,
            expected: FRAME_LENGTH_PREFIX_BYTES_V1,
            received: input.len(),
        });
    }

    let mut prefix = [0_u8; FRAME_LENGTH_PREFIX_BYTES_V1];
    prefix.copy_from_slice(&input[..FRAME_LENGTH_PREFIX_BYTES_V1]);
    let length = usize::try_from(u32::from_be_bytes(prefix))
        .map_err(|_| FrameErrorV1::InvalidLength(frame_length_conversion_error()))?;
    validate_frame_payload_length(length).map_err(FrameErrorV1::InvalidLength)?;

    let payload = &input[FRAME_LENGTH_PREFIX_BYTES_V1..];
    if payload.len() < length {
        return Err(FrameErrorV1::UnexpectedEof {
            phase: FrameIoPhaseV1::Payload,
            expected: length,
            received: payload.len(),
        });
    }
    if payload.len() > length {
        return Err(FrameErrorV1::TrailingData);
    }
    Ok(payload)
}

/// Reads one strict frame from a stream.
///
/// A clean EOF before a prefix returns `Ok(None)`. After a complete request frame is written,
/// request senders must half-close their write side so this function's EOF probe can distinguish a
/// complete request from data that may still arrive later.
pub fn read_single_frame_v1<R: Read>(reader: &mut R) -> Result<Option<Vec<u8>>, FrameErrorV1> {
    let mut prefix = [0_u8; FRAME_LENGTH_PREFIX_BYTES_V1];
    let received = read_until_eof(reader, &mut prefix, FrameIoPhaseV1::LengthPrefix)?;
    if received == 0 {
        return Ok(None);
    }
    if received < FRAME_LENGTH_PREFIX_BYTES_V1 {
        return Err(FrameErrorV1::UnexpectedEof {
            phase: FrameIoPhaseV1::LengthPrefix,
            expected: FRAME_LENGTH_PREFIX_BYTES_V1,
            received,
        });
    }

    let length = usize::try_from(u32::from_be_bytes(prefix))
        .map_err(|_| FrameErrorV1::InvalidLength(frame_length_conversion_error()))?;
    validate_frame_payload_length(length).map_err(FrameErrorV1::InvalidLength)?;

    let mut payload = vec![0_u8; length];
    let received = read_until_eof(reader, &mut payload, FrameIoPhaseV1::Payload)?;
    if received < length {
        return Err(FrameErrorV1::UnexpectedEof {
            phase: FrameIoPhaseV1::Payload,
            expected: length,
            received,
        });
    }

    let mut probe = [0_u8; 1];
    loop {
        match reader.read(&mut probe) {
            Ok(0) => return Ok(Some(payload)),
            Ok(_) => return Err(FrameErrorV1::TrailingData),
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(source) => {
                return Err(FrameErrorV1::Io {
                    phase: FrameIoPhaseV1::EndOfStream,
                    source,
                });
            }
        }
    }
}

/// Writes one bounded v1 frame without flushing the writer.
///
/// A strict receiver requires request senders to half-close their write side after this call.
pub fn write_frame_v1<W: Write>(writer: &mut W, payload: &[u8]) -> Result<(), FrameErrorV1> {
    let prefix = frame_prefix(payload.len()).map_err(FrameErrorV1::InvalidLength)?;
    write_all_phase(writer, &prefix, FrameIoPhaseV1::LengthPrefix)?;
    write_all_phase(writer, payload, FrameIoPhaseV1::Payload)
}

fn frame_prefix(length: usize) -> Result<[u8; FRAME_LENGTH_PREFIX_BYTES_V1], ProtocolError> {
    validate_frame_payload_length(length)?;
    let length = u32::try_from(length).map_err(|_| ProtocolError::FrameTooLarge {
        length,
        maximum: crate::MAX_FRAME_PAYLOAD_BYTES_V1,
    })?;
    Ok(length.to_be_bytes())
}

fn frame_length_conversion_error() -> ProtocolError {
    ProtocolError::FrameTooLarge {
        length: crate::MAX_FRAME_PAYLOAD_BYTES_V1 + 1,
        maximum: crate::MAX_FRAME_PAYLOAD_BYTES_V1,
    }
}

fn read_until_eof<R: Read>(
    reader: &mut R,
    buffer: &mut [u8],
    phase: FrameIoPhaseV1,
) -> Result<usize, FrameErrorV1> {
    let mut received = 0;
    while received < buffer.len() {
        match reader.read(&mut buffer[received..]) {
            Ok(0) => return Ok(received),
            Ok(count) if count <= buffer.len() - received => received += count,
            Ok(_) => {
                return Err(FrameErrorV1::Io {
                    phase,
                    source: io::Error::new(
                        io::ErrorKind::InvalidData,
                        "reader returned more bytes than the buffer can hold",
                    ),
                });
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(source) => return Err(FrameErrorV1::Io { phase, source }),
        }
    }
    Ok(received)
}

fn write_all_phase<W: Write>(
    writer: &mut W,
    bytes: &[u8],
    phase: FrameIoPhaseV1,
) -> Result<(), FrameErrorV1> {
    let mut remaining = bytes;
    while !remaining.is_empty() {
        match writer.write(remaining) {
            Ok(0) => {
                return Err(FrameErrorV1::Io {
                    phase,
                    source: io::Error::new(
                        io::ErrorKind::WriteZero,
                        "writer failed to make progress",
                    ),
                });
            }
            Ok(count) if count <= remaining.len() => remaining = &remaining[count..],
            Ok(_) => {
                return Err(FrameErrorV1::Io {
                    phase,
                    source: io::Error::new(
                        io::ErrorKind::InvalidData,
                        "writer reported more bytes than provided",
                    ),
                });
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(source) => return Err(FrameErrorV1::Io { phase, source }),
        }
    }
    Ok(())
}
