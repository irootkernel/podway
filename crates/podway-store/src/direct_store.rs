//! Generation-14 direct persistence.
//!
//! This boundary owns native maintenance leases and private SQLite transactions. The
//! engine supplies pure domain transitions and validates the complete public success.
//! It never exposes a SQLite connection or applies the daemon-era migration chain.

use std::{
    fmt, io,
    path::{Path, PathBuf},
};

use podway_core::{Sha256Digest, WorkspaceId};

use crate::{GraphMutationErrorV2, StoreErrorV1, ValidatedWorkspaceRootV1};

mod classification;
mod connection;
mod maintenance;
mod markers;
mod predecessor;
mod transaction;

pub use classification::{DirectStoreClassificationV1, classify_direct_store_v1};
pub use connection::{
    DirectPreparedGraphV1, DirectReadSnapshotV1, DirectStoreV1, DirectWorkspaceViewV1,
};
pub use maintenance::{ExclusiveMaintenanceLeaseV1, SharedMaintenanceLeaseV1};
pub use markers::{
    DirectMaintenanceMarkerKindV1, DirectMaintenanceMarkerV1, observe_direct_maintenance_marker_v1,
};
pub use predecessor::writer_predecessor_manifests_v1;
pub use transaction::{
    DirectMutationRequestV1, DirectMutationResultV1, DirectOutcomeScopeV1, DirectRecordedOutcomeV1,
    DirectTransactionBoundaryV1, DirectTransactionV1,
};

pub const DIRECT_SCHEMA_GENERATION_V1: u32 = 14;
pub const DIRECT_STORE_DDL_V1: &str = include_str!("../../../assets/specifications/sqlite-v14.sql");
pub const LEGACY_RUNTIME_MARKERS_V1: [&str; 3] = [
    "reset.marker",
    "workspace-removal.marker",
    "development-v2.marker",
];
pub const LEGACY_PODWAY_MARKER_V1: &str = "mode-switch.json";

/// Errors at the direct persistence, process and filesystem boundary.
#[derive(Debug)]
pub enum DirectStoreErrorV1 {
    NotInitialized,
    Maintenance,
    Busy,
    ConnectionBusy,
    ReadAdmissionBusy,
    Legacy {
        generation: Option<u32>,
        marker: Option<&'static str>,
    },
    UnsupportedGeneration {
        generation: u32,
    },
    Unreadable,
    WriterMismatch {
        expected: Sha256Digest,
        actual: String,
    },
    SchemaMismatch,
    BindingMismatch,
    ExpectedWorkspaceUuidMismatch {
        expected: WorkspaceId,
        actual: WorkspaceId,
    },
    IdempotencyKeyReused,
    MutationOutcomeUnknown,
    InvalidInput {
        reason: &'static str,
    },
    Domain(GraphMutationErrorV2),
    State(StoreErrorV1),
    Storage(io::Error),
    Sqlite(rusqlite::Error),
}

impl fmt::Display for DirectStoreErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Storage(error) => write!(formatter, "direct Store filesystem error: {error}"),
            Self::Sqlite(error) => write!(formatter, "direct Store SQLite error: {error}"),
            _ => write!(formatter, "direct Store boundary: {self:?}"),
        }
    }
}

impl std::error::Error for DirectStoreErrorV1 {}

impl From<io::Error> for DirectStoreErrorV1 {
    fn from(error: io::Error) -> Self {
        Self::Storage(error)
    }
}

impl From<StoreErrorV1> for DirectStoreErrorV1 {
    fn from(error: StoreErrorV1) -> Self {
        Self::State(error)
    }
}

impl From<GraphMutationErrorV2> for DirectStoreErrorV1 {
    fn from(error: GraphMutationErrorV2) -> Self {
        Self::Domain(error)
    }
}

impl From<rusqlite::Error> for DirectStoreErrorV1 {
    fn from(error: rusqlite::Error) -> Self {
        match error.sqlite_error_code() {
            Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked) => {
                Self::Busy
            }
            _ => Self::Sqlite(error),
        }
    }
}

pub type DirectStoreResultV1<T> = Result<T, DirectStoreErrorV1>;

/// Fixed worktree-local paths. The resolver supplies an absolute, lossless root.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirectWorkspaceLayoutV1 {
    root: ValidatedWorkspaceRootV1,
}

impl DirectWorkspaceLayoutV1 {
    pub fn new(root: ValidatedWorkspaceRootV1) -> DirectStoreResultV1<Self> {
        if !root.to_path_buf().is_absolute()
            || root.unix_bytes().len() > 4096
            || root.unix_bytes().contains(&0)
        {
            return Err(DirectStoreErrorV1::InvalidInput {
                reason: "worktree root must be absolute",
            });
        }
        Ok(Self { root })
    }

    pub fn root(&self) -> &ValidatedWorkspaceRootV1 {
        &self.root
    }
    pub fn podway_directory(&self) -> PathBuf {
        self.root.to_path_buf().join(".podway")
    }
    pub fn runtime_directory(&self) -> PathBuf {
        self.podway_directory().join("runtime")
    }
    pub fn database(&self) -> PathBuf {
        self.runtime_directory().join("state.sqlite3")
    }
    pub fn wal(&self) -> PathBuf {
        sidecar(&self.database(), "-wal")
    }
    pub fn shm(&self) -> PathBuf {
        sidecar(&self.database(), "-shm")
    }
    pub fn maintenance_lock(&self) -> PathBuf {
        self.runtime_directory().join("maintenance.lock")
    }
    pub fn intent_lock(&self) -> PathBuf {
        self.runtime_directory().join("maintenance-intent.lock")
    }
    pub fn intent_marker(&self) -> PathBuf {
        self.runtime_directory().join("maintenance-intent.json")
    }
    pub fn reset_marker(&self) -> PathBuf {
        self.runtime_directory().join("reset-all.marker")
    }
    pub fn removal_marker(&self) -> PathBuf {
        self.runtime_directory().join("removal.marker")
    }
}

fn sidecar(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    name.into()
}
