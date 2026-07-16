//! Same-user admission checks for accepted Unix-domain connections.
//!
//! Frame readers must enter through [`PeerUidVerifierV1::verify_before_frame`] so credential
//! lookup and UID comparison complete before any request bytes are consumed.

use std::{error::Error, fmt, io, os::unix::net::UnixStream};

use nix::{errno::Errno, unistd::geteuid};

/// Obtains a Unix-domain peer UID without reading application frames.
pub trait PeerCredentialSourceV1 {
    fn peer_uid(&self, connection: &UnixStream) -> Result<u32, PeerCredentialErrorV1>;
}

/// Errors while obtaining Unix-domain peer credentials.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PeerCredentialErrorV1 {
    /// The current target has no supported safe credential API in this build.
    UnsupportedPlatform,
    /// The platform credential API rejected the accepted Unix socket.
    Lookup(Errno),
}

impl fmt::Display for PeerCredentialErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedPlatform => {
                formatter.write_str("Unix-domain peer credentials are unsupported on this platform")
            }
            Self::Lookup(source) => write!(
                formatter,
                "cannot obtain Unix-domain peer credentials: {source}"
            ),
        }
    }
}

impl Error for PeerCredentialErrorV1 {}

/// The native credential source for the local operating system.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct NativePeerCredentialSourceV1;

impl PeerCredentialSourceV1 for NativePeerCredentialSourceV1 {
    fn peer_uid(&self, connection: &UnixStream) -> Result<u32, PeerCredentialErrorV1> {
        #[cfg(any(
            target_os = "macos",
            target_os = "ios",
            target_os = "freebsd",
            target_os = "openbsd",
            target_os = "netbsd",
            target_os = "dragonfly"
        ))]
        {
            nix::unistd::getpeereid(connection)
                .map(|(uid, _)| uid.as_raw())
                .map_err(PeerCredentialErrorV1::Lookup)
        }

        #[cfg(target_os = "linux")]
        {
            nix::sys::socket::getsockopt(connection, nix::sys::socket::sockopt::PeerCredentials)
                .map(|credentials| credentials.uid())
                .map_err(PeerCredentialErrorV1::Lookup)
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
            let _ = connection;
            Err(PeerCredentialErrorV1::UnsupportedPlatform)
        }
    }
}

/// A deterministic credential source for admission tests and non-native test harnesses.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FixedPeerCredentialSourceV1 {
    Uid(u32),
    Failure(PeerCredentialErrorV1),
}

impl FixedPeerCredentialSourceV1 {
    pub const fn uid(uid: u32) -> Self {
        Self::Uid(uid)
    }

    pub fn failure(error: PeerCredentialErrorV1) -> Self {
        Self::Failure(error)
    }
}

impl PeerCredentialSourceV1 for FixedPeerCredentialSourceV1 {
    fn peer_uid(&self, _connection: &UnixStream) -> Result<u32, PeerCredentialErrorV1> {
        match self {
            Self::Uid(uid) => Ok(*uid),
            Self::Failure(error) => Err(error.clone()),
        }
    }
}

/// An admission failure raised before a frame reader may inspect a connection.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PeerUidVerificationErrorV1 {
    Credential(PeerCredentialErrorV1),
    UidMismatch { expected_uid: u32, actual_uid: u32 },
}

impl fmt::Display for PeerUidVerificationErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Credential(source) => {
                write!(formatter, "peer credential lookup failed: {source}")
            }
            Self::UidMismatch {
                expected_uid,
                actual_uid,
            } => write!(
                formatter,
                "peer UID {actual_uid} does not match daemon UID {expected_uid}"
            ),
        }
    }
}

impl Error for PeerUidVerificationErrorV1 {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Credential(source) => Some(source),
            Self::UidMismatch { .. } => None,
        }
    }
}

/// The result of admitting a connection before a concrete frame reader runs.
#[derive(Debug)]
pub enum PeerFrameAdmissionErrorV1 {
    Peer(PeerUidVerificationErrorV1),
    FrameRead(io::Error),
}

impl fmt::Display for PeerFrameAdmissionErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Peer(source) => source.fmt(formatter),
            Self::FrameRead(source) => write!(
                formatter,
                "frame read failed after peer admission: {source}"
            ),
        }
    }
}

impl Error for PeerFrameAdmissionErrorV1 {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Peer(source) => Some(source),
            Self::FrameRead(source) => Some(source),
        }
    }
}

/// Verifies an accepted Unix-domain peer has the same UID as the daemon.
#[derive(Clone, Debug)]
pub struct PeerUidVerifierV1<Source> {
    expected_uid: u32,
    source: Source,
}

impl<Source> PeerUidVerifierV1<Source>
where
    Source: PeerCredentialSourceV1,
{
    pub const fn new(expected_uid: u32, source: Source) -> Self {
        Self {
            expected_uid,
            source,
        }
    }

    pub const fn expected_uid(&self) -> u32 {
        self.expected_uid
    }

    pub fn source(&self) -> &Source {
        &self.source
    }

    pub fn into_source(self) -> Source {
        self.source
    }

    /// Checks the native or injected peer credential without consuming frame bytes.
    pub fn verify(&self, connection: &UnixStream) -> Result<(), PeerUidVerificationErrorV1> {
        let actual_uid = self
            .source
            .peer_uid(connection)
            .map_err(PeerUidVerificationErrorV1::Credential)?;
        if actual_uid != self.expected_uid {
            return Err(PeerUidVerificationErrorV1::UidMismatch {
                expected_uid: self.expected_uid,
                actual_uid,
            });
        }
        Ok(())
    }

    /// Runs a frame reader only after [`Self::verify`] succeeds.
    pub fn verify_before_frame<T>(
        &self,
        connection: &UnixStream,
        read_frame: impl FnOnce(&UnixStream) -> io::Result<T>,
    ) -> Result<T, PeerFrameAdmissionErrorV1> {
        self.verify(connection)
            .map_err(PeerFrameAdmissionErrorV1::Peer)?;
        read_frame(connection).map_err(PeerFrameAdmissionErrorV1::FrameRead)
    }
}

impl PeerUidVerifierV1<NativePeerCredentialSourceV1> {
    /// Creates the production verifier for the effective UID of this daemon process.
    pub fn for_current_user() -> Self {
        Self::new(geteuid().as_raw(), NativePeerCredentialSourceV1)
    }
}
