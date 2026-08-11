#![forbid(unsafe_code)]

//! Durable-state contract for Podway workspaces.
//!
//! This crate intentionally models storage in domain terms. Public JSON and IPC
//! DTOs belong to `podway-protocol`, not to this contract.

use std::fmt;
use std::path::Path;
#[cfg(unix)]
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

#[cfg(unix)]
use std::os::unix::ffi::{OsStrExt, OsStringExt};

use podway_core::{
    AttemptId, DomainCommand, DomainError, DomainResult, ItemId, JobId, ProcedureSnapshotV1,
    Revision, SessionAggregateV1, SessionId, Sha256Digest, UnixMillis, WorkspaceId, WorkspaceState,
};
use serde_json::Value;

pub mod codec;
pub mod schema;
pub mod sqlite_store;
pub mod state_rows;
#[doc(hidden)]
pub mod test_support;
pub mod v2_goal;
pub mod v2_memory;
pub mod v2_state;

pub use codec::{
    PersistedGraphMutationFailureV2, PersistedGraphTerminalOperationV2,
    PersistedGraphTerminalSessionProjectionV2, PersistedResponseContextV1,
    PersistedStartIdentityV1, PersistedTerminalJobProjectionV1, PersistedTerminalJobStateV1,
    PersistedTerminalReceiptV1, PersistedTerminalSessionProjectionV1,
};
pub use sqlite_store::SqliteStoreV1;
pub use v2_goal::{AttemptCriterionAssessmentStateV2, CriterionAssessmentStateV2, GoalStateV2};
pub use v2_memory::{
    ActiveItemMutationV2, AttemptWorkflowMemoryV2, BlockerStateV2, EvidenceReadbackV2,
    EvidenceResolutionStateV2, GraphMutationErrorV2, ItemSlotStateV2, MAX_OPEN_BLOCKERS_V2,
    WorkflowMemoryStateV2, canonical_recorded_items_json_v2, recorded_items_digest_v2,
};
pub use v2_state::{
    AttemptMetadataV2, GraphActionCompletionOutcomeV2, GraphActionSkipOutcomeV2,
    GraphBlockOutcomeV2, GraphCancelOutcomeV2, GraphCriterionAssessmentOutcomeV2,
    GraphGoalDefinitionOutcomeV2, GraphGoalRevisionOutcomeV2, GraphItemMutationOutcomeV2,
    GraphNodeCounterV2, GraphNodeSnapshotV2, GraphRetryOutcomeV2, GraphSessionStateV2,
    GraphStartCurrentTaskV2, GraphUnblockOutcomeV2, GraphWorkspaceViewV2, ProcedureSnapshotV2,
    StoreGraphMutationContractV2, StoreGraphReadContractV2, StoreGraphStateContractV2,
};

pub const MAX_IDEMPOTENCY_KEY_BYTES_V1: usize = 256;
pub const MAX_WORKER_ID_BYTES_V1: usize = 128;
pub const MAX_CANONICAL_EXECUTION_JSON_BYTES_V1: usize = 1_048_576;
pub const MAX_CANONICAL_EXECUTION_JSON_DEPTH_V1: usize = 64;
pub const MAX_JOB_LIST_LIMIT_V1: u32 = 1_000;

/// Core identifiers and values named by the Store v1 boundary.
pub type CanonicalRequestDigestV1 = Sha256Digest;
pub type CommandV1 = DomainCommand;
pub type EpochMillisV1 = UnixMillis;
pub type GitIdentityV1 = Sha256Digest;
pub type TerminalEnvelopeSealerV1 = fn(&PersistedTerminalReceiptV1) -> Result<Value, StoreErrorV1>;

/// Identifies the runtime model encoded by one complete durable execution document.
///
/// Legacy constructors remain v1 so their canonical Store encoding is unchanged. Procedure v2
/// callers must opt in explicitly; the flavor then survives admission, claim, and restart.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum DurableExecutionFlavorV1 {
    #[default]
    LegacyV1,
    ProcedureV2,
}

static TERMINAL_ENVELOPE_SEALER_V1: OnceLock<TerminalEnvelopeSealerV1> = OnceLock::new();

/// Installs the daemon-owned pure renderer used inside terminal Store transactions.
pub fn install_terminal_envelope_sealer_v1(sealer: TerminalEnvelopeSealerV1) {
    let _ = TERMINAL_ENVELOPE_SEALER_V1.set(sealer);
}

pub(crate) fn terminal_envelope_sealer_v1() -> Option<TerminalEnvelopeSealerV1> {
    TERMINAL_ENVELOPE_SEALER_V1.get().copied()
}
pub type JobIdV1 = JobId;
pub type RevisionV1 = Revision;
pub type WorkspaceUuidV1 = WorkspaceId;
pub const DEFAULT_SQLITE_BUSY_TIMEOUT_MS_V1: u32 = 5_000;
pub const MAX_SQLITE_BUSY_TIMEOUT_MS_V1: u32 = 5_000;
pub const WORKSPACE_ROOT_TEXT_PREFIX_V1: &str = "podway.unix-path/v1:";
pub(crate) fn command_name_v1(command: &CommandV1) -> &'static str {
    match command {
        CommandV1::WorkspaceInitialize => "workspace.initialize",
        CommandV1::WorkspaceResetAll => "workspace.reset_all",
        CommandV1::SessionStart => "session.start",
        CommandV1::SessionStartReplace => "session.start_replace",
        CommandV1::SessionComplete => "session.complete",
        CommandV1::SessionSkip => "session.skip",
        CommandV1::SessionRetry => "session.retry",
        CommandV1::SessionReturn => "session.return",
        CommandV1::SessionBlock => "session.block",
        CommandV1::SessionUnblock => "session.unblock",
        CommandV1::SessionCancel => "session.cancel",
        CommandV1::SessionReopen => "session.reopen",
        CommandV1::SessionReset => "session.reset",
        CommandV1::SessionDecide => "session.decide",
        CommandV1::SessionRework => "session.rework",
        CommandV1::GoalDefine => "goal.define",
        CommandV1::GoalRevise => "goal.revise",
        CommandV1::GoalAssessCriterion => "goal.assess_criterion",
        CommandV1::ItemCheck { .. } => "item.check",
        CommandV1::ItemUncheck { .. } => "item.uncheck",
        CommandV1::ItemSet { .. } => "item.set",
        CommandV1::ItemAdd { .. } => "item.add",
        CommandV1::ItemRemove { .. } => "item.remove",
        CommandV1::ItemAttach { .. } => "item.attach",
        CommandV1::ItemClear { .. } => "item.clear",
    }
}
pub(crate) fn command_is_session_scoped_v1(command: &CommandV1) -> bool {
    matches!(
        command,
        CommandV1::SessionComplete
            | CommandV1::SessionSkip
            | CommandV1::SessionRetry
            | CommandV1::SessionReturn
            | CommandV1::SessionBlock
            | CommandV1::SessionUnblock
            | CommandV1::SessionCancel
            | CommandV1::SessionReopen
            | CommandV1::SessionDecide
            | CommandV1::SessionRework
            | CommandV1::GoalDefine
            | CommandV1::GoalRevise
            | CommandV1::GoalAssessCriterion
            | CommandV1::ItemCheck { .. }
            | CommandV1::ItemUncheck { .. }
            | CommandV1::ItemSet { .. }
            | CommandV1::ItemAdd { .. }
            | CommandV1::ItemRemove { .. }
            | CommandV1::ItemAttach { .. }
            | CommandV1::ItemClear { .. }
    )
}
/// A bounded, exact canonical JSON document needed to execute a durable command after restart.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CanonicalExecutionJsonV1(String);

impl CanonicalExecutionJsonV1 {
    pub fn new(value: impl Into<String>) -> Result<Self, StoreValueErrorV1> {
        let value = value.into();
        if value.is_empty() {
            return Err(StoreValueErrorV1::EmptyValue {
                field: "canonical execution JSON",
            });
        }
        if value.len() > MAX_CANONICAL_EXECUTION_JSON_BYTES_V1 {
            return Err(StoreValueErrorV1::ValueTooLong {
                field: "canonical execution JSON",
                maximum_bytes: MAX_CANONICAL_EXECUTION_JSON_BYTES_V1,
            });
        }

        let document: serde_json::Value = serde_json::from_str(&value)
            .map_err(|_| StoreValueErrorV1::InvalidCanonicalExecutionJson)?;
        validate_canonical_execution_depth_v1(&document, 0)?;
        podway_core::verify_canonical_json_v1(value.as_bytes())
            .map_err(|_| StoreValueErrorV1::InvalidCanonicalExecutionJson)?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn into_inner(self) -> String {
        self.0
    }
}

fn validate_canonical_execution_depth_v1(
    value: &serde_json::Value,
    depth: usize,
) -> Result<(), StoreValueErrorV1> {
    if depth > MAX_CANONICAL_EXECUTION_JSON_DEPTH_V1 {
        return Err(StoreValueErrorV1::CanonicalExecutionJsonDepthExceeded {
            maximum: MAX_CANONICAL_EXECUTION_JSON_DEPTH_V1,
        });
    }

    match value {
        serde_json::Value::Array(values) => {
            for value in values {
                validate_canonical_execution_depth_v1(value, depth + 1)?;
            }
        }
        serde_json::Value::Object(values) => {
            for value in values.values() {
                validate_canonical_execution_depth_v1(value, depth + 1)?;
            }
        }
        serde_json::Value::Null
        | serde_json::Value::Bool(_)
        | serde_json::Value::Number(_)
        | serde_json::Value::String(_) => {}
    }
    Ok(())
}

fn legacy_minimal_canonical_execution_v1(command: &CommandV1) -> CanonicalExecutionJsonV1 {
    let command_name = command_name_v1(command);
    let document = match command {
        CommandV1::ItemCheck { item_id }
        | CommandV1::ItemUncheck { item_id }
        | CommandV1::ItemSet { item_id }
        | CommandV1::ItemAdd { item_id }
        | CommandV1::ItemRemove { item_id }
        | CommandV1::ItemAttach { item_id }
        | CommandV1::ItemClear { item_id } => {
            serde_json::json!({"command": command_name, "item_id": item_id.as_str()})
        }
        _ => serde_json::json!({"command": command_name}),
    };
    let canonical = podway_core::canonicalize_json_v1(&document)
        .expect("legacy command documents must be canonicalizable");
    CanonicalExecutionJsonV1::new(canonical)
        .expect("legacy command documents must satisfy execution bounds")
}

fn direct_store_canonical_execution_v1(
    command: &CommandV1,
    preconditions: &RevisionAttemptItemPreconditionsV1,
) -> CanonicalExecutionJsonV1 {
    let execution = ClaimedExecutionV1::new(command.clone(), preconditions.clone());
    let canonical = crate::codec::encode_command_v1(&execution)
        .expect("direct Store command documents must be canonicalizable");
    CanonicalExecutionJsonV1::new(canonical)
        .expect("direct Store command documents must satisfy execution bounds")
}

/// Lossless validated workspace-root evidence stored in `workspace_state.last_validated_root`.
///
/// The canonical text is a fixed prefix followed by lowercase hexadecimal Unix path bytes.
/// It is deliberately not a display path and must never be produced with lossy UTF-8 conversion.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedWorkspaceRootV1 {
    bytes: Vec<u8>,
    encoded: String,
}

impl ValidatedWorkspaceRootV1 {
    #[cfg(unix)]
    pub fn from_path(path: &Path) -> Result<Self, StoreValueErrorV1> {
        Self::from_unix_bytes(path.as_os_str().as_bytes().to_vec())
    }

    #[cfg(not(unix))]
    pub fn from_path(_path: &Path) -> Result<Self, StoreValueErrorV1> {
        Err(StoreValueErrorV1::UnsupportedWorkspaceRootPlatform)
    }

    #[cfg(unix)]
    pub fn from_unix_bytes(bytes: Vec<u8>) -> Result<Self, StoreValueErrorV1> {
        if bytes.is_empty() {
            return Err(StoreValueErrorV1::InvalidWorkspaceRootEncoding);
        }
        Ok(Self {
            encoded: encode_lower_hex(&bytes),
            bytes,
        })
    }

    pub fn from_encoded(encoded: impl Into<String>) -> Result<Self, StoreValueErrorV1> {
        let encoded = encoded.into();
        let hex = encoded
            .strip_prefix(WORKSPACE_ROOT_TEXT_PREFIX_V1)
            .ok_or(StoreValueErrorV1::InvalidWorkspaceRootEncoding)?;
        let bytes = decode_lower_hex(hex)?;
        if bytes.is_empty() {
            return Err(StoreValueErrorV1::InvalidWorkspaceRootEncoding);
        }
        if encode_lower_hex(&bytes) != encoded {
            return Err(StoreValueErrorV1::InvalidWorkspaceRootEncoding);
        }
        Ok(Self { bytes, encoded })
    }

    pub fn as_encoded(&self) -> &str {
        &self.encoded
    }

    pub fn unix_bytes(&self) -> &[u8] {
        &self.bytes
    }

    #[cfg(unix)]
    pub fn to_path_buf(&self) -> PathBuf {
        PathBuf::from(std::ffi::OsString::from_vec(self.bytes.clone()))
    }
}

/// Runtime limits and explicit test fault controls for the SQLite store.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SqliteStoreOptionsV1 {
    max_pending_jobs: u32,
    busy_timeout_ms: u32,
    failpoint: Option<StoreFailpointV1>,
    failpoint_action: StoreFailpointActionV1,
}

impl SqliteStoreOptionsV1 {
    pub fn new(max_pending_jobs: u32) -> Result<Self, StoreValueErrorV1> {
        if max_pending_jobs == 0 {
            return Err(StoreValueErrorV1::ZeroValue {
                field: "max pending jobs",
            });
        }
        Ok(Self {
            max_pending_jobs,
            busy_timeout_ms: DEFAULT_SQLITE_BUSY_TIMEOUT_MS_V1,
            failpoint: None,
            failpoint_action: StoreFailpointActionV1::ReturnError,
        })
    }

    pub fn with_busy_timeout_ms(mut self, busy_timeout_ms: u32) -> Result<Self, StoreValueErrorV1> {
        if busy_timeout_ms == 0 || busy_timeout_ms > MAX_SQLITE_BUSY_TIMEOUT_MS_V1 {
            return Err(StoreValueErrorV1::BusyTimeoutOutOfRange {
                maximum_ms: MAX_SQLITE_BUSY_TIMEOUT_MS_V1,
            });
        }
        self.busy_timeout_ms = busy_timeout_ms;
        Ok(self)
    }

    pub fn with_failpoint(mut self, failpoint: Option<StoreFailpointV1>) -> Self {
        self.failpoint = failpoint;
        self
    }

    /// Selects the explicit action taken when the configured failpoint is reached.
    ///
    /// `AbortProcess` is intended solely for isolated child-process crash fixtures. `Barrier`
    /// is a test-only rendezvous that resumes normally once every configured participant arrives.
    pub fn with_failpoint_action(mut self, action: StoreFailpointActionV1) -> Self {
        self.failpoint_action = action;
        self
    }

    pub fn max_pending_jobs(&self) -> u32 {
        self.max_pending_jobs
    }

    pub fn busy_timeout_ms(&self) -> u32 {
        self.busy_timeout_ms
    }

    pub fn failpoint(&self) -> Option<StoreFailpointV1> {
        self.failpoint
    }

    pub(crate) fn trigger_failpoint(
        &self,
        failpoint: StoreFailpointV1,
    ) -> Result<(), StoreErrorV1> {
        if self.failpoint != Some(failpoint) {
            return Ok(());
        }
        match &self.failpoint_action {
            StoreFailpointActionV1::ReturnError => Err(StoreErrorV1::StorageUnavailableV1 {
                reason: StoreUnavailableReasonV1::Recovery,
            }),
            StoreFailpointActionV1::ReturnInjectedStorageIo => {
                Err(StoreErrorV1::StorageUnavailableV1 {
                    reason: StoreUnavailableReasonV1::StorageIo,
                })
            }
            StoreFailpointActionV1::AbortProcess => std::process::abort(),
            StoreFailpointActionV1::Barrier(barrier) => {
                barrier.wait();
                Ok(())
            }
        }
    }
}

/// Injection points reserved for deterministic concrete-store failure tests.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StoreFailpointV1 {
    SchemaAfterPragmas,
    SchemaAfterPragmasAndTemporaryCleanup,
    SchemaBeforeCommit,
    /// Reached after an initialized temporary database is durable and before its no-clobber
    /// publication attempt.
    SchemaAfterInitializationBeforePublication,
    /// Reached after a destination hard link has been durably verified and before the
    /// Store-owned temporary link is unlinked. Shared by ordinary and reset publication.
    PublicationAfterDestinationLinkBeforeTemporaryUnlink,
    AdmissionBeforeTransaction,
    AdmissionAfterDurableRowsBeforeCommit,
    AdmissionAfterCommit,
    ClaimAfterCommit,
    TerminalAfterTransactionBegin,
    TerminalAfterRelationalStateUpdatesBeforeJobTerminalUpdate,
    TerminalAfterJobTerminalUpdateBeforeCommit,
    /// Retained for existing callers that need the final terminal pre-commit seam.
    TerminalBeforeCommit,
    TerminalAfterCommitBeforeResponse,
    TerminalFailureBeforeCommit,
    PruneAfterDeleteStagingBeforeCommit,
    RecoveryBeforeCommit,
    RecoveryAfterCommitBeforeReturn,
    ResetBeforeSeedCommit,
    ResetBeforeSeedCommitAndTemporaryCleanup,
    ResetAfterSeedCommitBeforePublication,
    ResetAfterPublicationBeforeResponse,
    ResetAfterPublicationBeforeResponseAndTemporaryCleanup,
    V2GraphStateBeforeCommit,
}

/// Explicit behavior for a configured Store test failpoint.
#[derive(Clone)]
pub enum StoreFailpointActionV1 {
    /// Preserve ordinary Store error behavior.
    ReturnError,
    /// Inject the public storage-I/O classification at the selected pre-commit seam.
    ///
    /// This action verifies transactional rollback and error propagation. It does not claim to
    /// emulate SQLite VFS behavior or physical storage exhaustion.
    ReturnInjectedStorageIo,
    /// Abort immediately at the configured failpoint; use only in child-process fixtures.
    AbortProcess,
    /// Wait for every participant at this test-only rendezvous, then continue normally.
    Barrier(std::sync::Arc<std::sync::Barrier>),
}

impl fmt::Debug for StoreFailpointActionV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReturnError => formatter.write_str("ReturnError"),
            Self::ReturnInjectedStorageIo => formatter.write_str("ReturnInjectedStorageIo"),
            Self::AbortProcess => formatter.write_str("AbortProcess"),
            Self::Barrier(_) => formatter.write_str("Barrier(..)"),
        }
    }
}

impl PartialEq for StoreFailpointActionV1 {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::ReturnError, Self::ReturnError)
            | (Self::ReturnInjectedStorageIo, Self::ReturnInjectedStorageIo)
            | (Self::AbortProcess, Self::AbortProcess) => true,
            (Self::Barrier(left), Self::Barrier(right)) => std::sync::Arc::ptr_eq(left, right),
            _ => false,
        }
    }
}

impl Eq for StoreFailpointActionV1 {}

/// Durability classification for a Phase-2 crash boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StoreCrashBoundaryDurabilityV1 {
    PreCommitRollback,
    PostCommitReplay,
    DaemonPreparation,
}

/// Source-visible Phase-2 crash-boundary coverage record.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StoreCrashBoundaryV1 {
    id: &'static str,
    failpoints: &'static [StoreFailpointV1],
    durability: StoreCrashBoundaryDurabilityV1,
    recovery_invariant: &'static str,
    requirements: &'static [&'static str],
}

impl StoreCrashBoundaryV1 {
    pub const fn id(&self) -> &'static str {
        self.id
    }

    pub const fn failpoints(&self) -> &'static [StoreFailpointV1] {
        self.failpoints
    }

    pub const fn durability(&self) -> StoreCrashBoundaryDurabilityV1 {
        self.durability
    }

    pub const fn recovery_invariant(&self) -> &'static str {
        self.recovery_invariant
    }

    pub const fn requirements(&self) -> &'static [&'static str] {
        self.requirements
    }
}

/// Store-owned crash registry for C01-C13 plus publication boundary P01.
/// C05/C06 are daemon preparation boundaries deliberately outside Store transactions;
/// reset-all boundaries C14-C16 are daemon runtime boundaries.
pub const PHASE2_CRASH_BOUNDARY_REGISTRY_V1: &[StoreCrashBoundaryV1] = &[
    StoreCrashBoundaryV1 {
        id: "C01",
        failpoints: &[StoreFailpointV1::AdmissionBeforeTransaction],
        durability: StoreCrashBoundaryDurabilityV1::PreCommitRollback,
        recovery_invariant: "no admission rows exist",
        requirements: &["STO-001", "STO-003"],
    },
    StoreCrashBoundaryV1 {
        id: "C02",
        failpoints: &[StoreFailpointV1::AdmissionAfterDurableRowsBeforeCommit],
        durability: StoreCrashBoundaryDurabilityV1::PreCommitRollback,
        recovery_invariant: "no job or idempotency record exists",
        requirements: &["STO-001", "STO-003"],
    },
    StoreCrashBoundaryV1 {
        id: "C03",
        failpoints: &[StoreFailpointV1::AdmissionAfterCommit],
        durability: StoreCrashBoundaryDurabilityV1::PostCommitReplay,
        recovery_invariant: "one queued job replays by idempotency",
        requirements: &["STO-001", "STO-003"],
    },
    StoreCrashBoundaryV1 {
        id: "C04",
        failpoints: &[StoreFailpointV1::ClaimAfterCommit],
        durability: StoreCrashBoundaryDurabilityV1::PostCommitReplay,
        recovery_invariant: "one running job is requeued once on restart",
        requirements: &["STO-002", "STO-003"],
    },
    StoreCrashBoundaryV1 {
        id: "C05",
        failpoints: &[],
        durability: StoreCrashBoundaryDurabilityV1::DaemonPreparation,
        recovery_invariant: "procedure preparation is daemon-owned",
        requirements: &[],
    },
    StoreCrashBoundaryV1 {
        id: "C06",
        failpoints: &[],
        durability: StoreCrashBoundaryDurabilityV1::DaemonPreparation,
        recovery_invariant: "artifact hashing is daemon-owned",
        requirements: &[],
    },
    StoreCrashBoundaryV1 {
        id: "C07",
        failpoints: &[StoreFailpointV1::TerminalAfterTransactionBegin],
        durability: StoreCrashBoundaryDurabilityV1::PreCommitRollback,
        recovery_invariant: "claimed job remains recoverable",
        requirements: &["STO-002", "STO-003"],
    },
    StoreCrashBoundaryV1 {
        id: "C08",
        failpoints: &[StoreFailpointV1::TerminalAfterRelationalStateUpdatesBeforeJobTerminalUpdate],
        durability: StoreCrashBoundaryDurabilityV1::PreCommitRollback,
        recovery_invariant: "relational state and job terminal receipt roll back together",
        requirements: &["STO-002", "STO-003"],
    },
    StoreCrashBoundaryV1 {
        id: "C09",
        failpoints: &[StoreFailpointV1::TerminalAfterJobTerminalUpdateBeforeCommit],
        durability: StoreCrashBoundaryDurabilityV1::PreCommitRollback,
        recovery_invariant: "job and idempotency terminal updates roll back together",
        requirements: &["STO-002", "STO-003"],
    },
    StoreCrashBoundaryV1 {
        id: "C10",
        failpoints: &[
            StoreFailpointV1::TerminalAfterCommitBeforeResponse,
            StoreFailpointV1::RecoveryAfterCommitBeforeReturn,
        ],
        durability: StoreCrashBoundaryDurabilityV1::PostCommitReplay,
        recovery_invariant: "one committed outcome is replayable after a lost response",
        requirements: &["STO-002", "STO-003"],
    },
    StoreCrashBoundaryV1 {
        id: "C11",
        failpoints: &[StoreFailpointV1::TerminalFailureBeforeCommit],
        durability: StoreCrashBoundaryDurabilityV1::PreCommitRollback,
        recovery_invariant: "failed receipt commits once on retry without domain mutation",
        requirements: &["STO-002", "STO-003"],
    },
    StoreCrashBoundaryV1 {
        id: "C12",
        failpoints: &[StoreFailpointV1::PruneAfterDeleteStagingBeforeCommit],
        durability: StoreCrashBoundaryDurabilityV1::PreCommitRollback,
        recovery_invariant: "prune deletes roll back with the caller transaction",
        requirements: &["STO-003", "STO-009"],
    },
    StoreCrashBoundaryV1 {
        id: "C13",
        failpoints: &[StoreFailpointV1::SchemaBeforeCommit],
        durability: StoreCrashBoundaryDurabilityV1::PreCommitRollback,
        recovery_invariant: "migration either commits once or leaves no partial schema",
        requirements: &["STO-007"],
    },
    StoreCrashBoundaryV1 {
        id: "P01",
        failpoints: &[StoreFailpointV1::PublicationAfterDestinationLinkBeforeTemporaryUnlink],
        durability: StoreCrashBoundaryDurabilityV1::PostCommitReplay,
        recovery_invariant: "durable destination wins and the matching Store temporary hard link is removed",
        requirements: &["STO-007", "STO-008"],
    },
];

/// Scope selected for a storage integrity pass.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IntegrityModeV1 {
    Fast,
    Deep,
}

/// One completed integrity assertion.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IntegrityCheckResultV1 {
    check: StoreIntegrityCheckV1,
    passed: bool,
}

impl IntegrityCheckResultV1 {
    pub fn new(check: StoreIntegrityCheckV1, passed: bool) -> Self {
        Self { check, passed }
    }

    pub fn check(&self) -> &StoreIntegrityCheckV1 {
        &self.check
    }

    pub fn passed(&self) -> bool {
        self.passed
    }
}

/// Results produced by a completed storage integrity operation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IntegrityReportV1 {
    mode: IntegrityModeV1,
    checked_at: EpochMillisV1,
    checks: Vec<IntegrityCheckResultV1>,
}

impl IntegrityReportV1 {
    pub fn new(
        mode: IntegrityModeV1,
        checked_at: EpochMillisV1,
        checks: Vec<IntegrityCheckResultV1>,
    ) -> Self {
        Self {
            mode,
            checked_at,
            checks,
        }
    }

    pub fn mode(&self) -> IntegrityModeV1 {
        self.mode
    }

    pub fn checked_at(&self) -> EpochMillisV1 {
        self.checked_at
    }

    pub fn checks(&self) -> &[IntegrityCheckResultV1] {
        &self.checks
    }
}

/// Summary of startup recovery performed before the store became available.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoveryReportV1 {
    requeued_job_count: u32,
    recovered_at: EpochMillisV1,
}

impl RecoveryReportV1 {
    pub fn new(requeued_job_count: u32, recovered_at: EpochMillisV1) -> Self {
        Self {
            requeued_job_count,
            recovered_at,
        }
    }

    pub fn requeued_job_count(&self) -> u32 {
        self.requeued_job_count
    }

    pub fn recovered_at(&self) -> EpochMillisV1 {
        self.recovered_at
    }
}

/// Counts returned after terminal-history pruning.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PruneReportV1 {
    deleted_terminal_jobs: u32,
    deleted_journal_entries: u32,
    deleted_orphan_workspace_receipts: u32,
    pruned_at: EpochMillisV1,
}

impl PruneReportV1 {
    pub fn new(
        deleted_terminal_jobs: u32,
        deleted_journal_entries: u32,
        deleted_orphan_workspace_receipts: u32,
        pruned_at: EpochMillisV1,
    ) -> Self {
        Self {
            deleted_terminal_jobs,
            deleted_journal_entries,
            deleted_orphan_workspace_receipts,
            pruned_at,
        }
    }

    pub fn deleted_terminal_jobs(&self) -> u32 {
        self.deleted_terminal_jobs
    }

    pub fn deleted_journal_entries(&self) -> u32 {
        self.deleted_journal_entries
    }

    pub fn deleted_orphan_workspace_receipts(&self) -> u32 {
        self.deleted_orphan_workspace_receipts
    }

    pub fn pruned_at(&self) -> EpochMillisV1 {
        self.pruned_at
    }
}

/// The synchronous persistence boundary used by the daemon scheduler.
///
/// Each successful method call observes one coherent committed workspace state.
pub trait StoreContractV1: Send + Sync {
    fn admit(
        &self,
        identity: &DurableWorktreeIdentityV1,
        request: AdmitRequestV1,
    ) -> Result<AdmitOutcomeV1, StoreErrorV1>;

    fn claim_next(
        &self,
        identity: &DurableWorktreeIdentityV1,
        worker: WorkerIdV1,
        now: EpochMillisV1,
    ) -> Result<Option<ClaimedJobV1>, StoreErrorV1>;

    fn cancel_before_claim(
        &self,
        identity: &DurableWorktreeIdentityV1,
        job: JobIdV1,
        expected_job_revision: RevisionV1,
        now: EpochMillisV1,
    ) -> Result<CancelOutcomeV1, StoreErrorV1>;

    fn commit_terminal(
        &self,
        claim: ClaimTokenV1,
        expected_workspace_revision: RevisionV1,
        transition: Option<StateTransitionV1>,
        result: TerminalResultV1,
        now: EpochMillisV1,
    ) -> Result<TerminalReceiptV1, StoreErrorV1>;

    fn read_workspace_view(
        &self,
        identity: &DurableWorktreeIdentityV1,
    ) -> Result<WorkspaceViewV1, StoreErrorV1>;
}

/// Additive coherent-read capability for storage consumers that need durable domain facts.
pub trait StoreReadContractV1: StoreContractV1 {
    fn read_session_aggregate(
        &self,
        identity: &DurableWorktreeIdentityV1,
    ) -> Result<Option<SessionAggregateV1>, StoreErrorV1>;

    fn read_job(
        &self,
        identity: &DurableWorktreeIdentityV1,
        job: &JobIdV1,
    ) -> Result<Option<JobViewV1>, StoreErrorV1>;

    fn list_jobs(
        &self,
        identity: &DurableWorktreeIdentityV1,
        query: JobListQueryV1,
    ) -> Result<Vec<JobViewV1>, StoreErrorV1>;
}

/// Additive pre-admission lookup used to return immutable idempotent outcomes before any fresh
/// execution dependencies are consulted.
pub trait StoreIdempotencyReadContractV1: StoreContractV1 {
    /// Looks up the durable job binding for an idempotency key without admitting or replaying a
    /// mutation.
    fn read_idempotency_lookup(
        &self,
        identity: &DurableWorktreeIdentityV1,
        idempotency_key: &IdempotencyKeyV1,
    ) -> Result<Option<IdempotencyLookupV1>, StoreErrorV1> {
        let Some(execution) = self.read_idempotent_execution(identity, idempotency_key)? else {
            return Ok(None);
        };
        let (job_id, terminal_receipt) = match execution.outcome() {
            AdmitOutcomeV1::New(receipt)
            | AdmitOutcomeV1::Existing(JobReceiptOrTerminalV1::JobReceipt(receipt)) => {
                (receipt.job_id().clone(), None)
            }
            AdmitOutcomeV1::Existing(JobReceiptOrTerminalV1::TerminalReceipt(receipt)) => {
                (receipt.job().job_id().clone(), Some(receipt.clone()))
            }
        };
        Ok(Some(IdempotencyLookupV1::new_with_terminal_receipt(
            job_id,
            execution.request_digest().clone(),
            terminal_receipt,
        )))
    }

    fn read_idempotent_outcome(
        &self,
        identity: &DurableWorktreeIdentityV1,
        idempotency_key: &IdempotencyKeyV1,
        request_digest: &CanonicalRequestDigestV1,
    ) -> Result<Option<AdmitOutcomeV1>, StoreErrorV1>;

    /// Reads the immutable execution bound to a key so callers can reconstruct a start identity
    /// from its admitted Procedure digest before consulting fresh source dependencies.
    fn read_idempotent_execution(
        &self,
        identity: &DurableWorktreeIdentityV1,
        idempotency_key: &IdempotencyKeyV1,
    ) -> Result<Option<IdempotentExecutionV1>, StoreErrorV1>;
}

/// One transactionally coherent view used to reconcile an idempotency key without admission.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReconciliationSnapshotV1 {
    latest_workspace_sequence: u64,
    lookup: Option<IdempotencyLookupV1>,
    job: Option<JobViewV1>,
}

impl ReconciliationSnapshotV1 {
    pub fn new(
        latest_workspace_sequence: u64,
        lookup: Option<IdempotencyLookupV1>,
        job: Option<JobViewV1>,
    ) -> Self {
        Self {
            latest_workspace_sequence,
            lookup,
            job,
        }
    }

    pub const fn latest_workspace_sequence(&self) -> u64 {
        self.latest_workspace_sequence
    }

    pub fn lookup(&self) -> Option<&IdempotencyLookupV1> {
        self.lookup.as_ref()
    }

    pub fn job(&self) -> Option<&JobViewV1> {
        self.job.as_ref()
    }
}

/// Additive coherent read used by `job.lookup` reconciliation.
pub trait StoreReconciliationReadContractV1: StoreContractV1 {
    fn read_reconciliation_snapshot(
        &self,
        identity: &DurableWorktreeIdentityV1,
        idempotency_key: &IdempotencyKeyV1,
    ) -> Result<ReconciliationSnapshotV1, StoreErrorV1>;
}

/// Minimal read-only binding exposed to idempotency reconciliation callers.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IdempotencyLookupV1 {
    job_id: JobIdV1,
    request_digest: CanonicalRequestDigestV1,
    terminal_receipt: Option<PersistedTerminalReceiptV1>,
}

impl IdempotencyLookupV1 {
    pub fn new(job_id: JobIdV1, request_digest: CanonicalRequestDigestV1) -> Self {
        Self {
            job_id,
            request_digest,
            terminal_receipt: None,
        }
    }

    pub fn new_with_terminal_receipt(
        job_id: JobIdV1,
        request_digest: CanonicalRequestDigestV1,
        terminal_receipt: Option<PersistedTerminalReceiptV1>,
    ) -> Self {
        Self {
            job_id,
            request_digest,
            terminal_receipt,
        }
    }

    pub fn job_id(&self) -> &JobIdV1 {
        &self.job_id
    }

    pub fn request_digest(&self) -> &CanonicalRequestDigestV1 {
        &self.request_digest
    }

    pub fn terminal_receipt(&self) -> Option<&PersistedTerminalReceiptV1> {
        self.terminal_receipt.as_ref()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IdempotentExecutionV1 {
    canonical_execution: Option<CanonicalExecutionJsonV1>,
    retained_start_identity: Option<PersistedStartIdentityV1>,
    outcome: AdmitOutcomeV1,
    request_digest: CanonicalRequestDigestV1,
}

impl IdempotentExecutionV1 {
    pub fn new(
        request_digest: CanonicalRequestDigestV1,
        canonical_execution: Option<CanonicalExecutionJsonV1>,
        retained_start_identity: Option<PersistedStartIdentityV1>,
        outcome: AdmitOutcomeV1,
    ) -> Self {
        Self {
            canonical_execution,
            retained_start_identity,
            outcome,
            request_digest,
        }
    }

    pub fn request_digest(&self) -> &CanonicalRequestDigestV1 {
        &self.request_digest
    }

    pub fn canonical_execution(&self) -> Option<&CanonicalExecutionJsonV1> {
        self.canonical_execution.as_ref()
    }

    pub fn retained_start_identity(&self) -> Option<&PersistedStartIdentityV1> {
        self.retained_start_identity.as_ref()
    }

    pub fn outcome(&self) -> &AdmitOutcomeV1 {
        &self.outcome
    }
}

impl<Store> StoreContractV1 for Arc<Store>
where
    Store: StoreContractV1 + ?Sized,
{
    fn admit(
        &self,
        identity: &DurableWorktreeIdentityV1,
        request: AdmitRequestV1,
    ) -> Result<AdmitOutcomeV1, StoreErrorV1> {
        (**self).admit(identity, request)
    }

    fn claim_next(
        &self,
        identity: &DurableWorktreeIdentityV1,
        worker: WorkerIdV1,
        now: EpochMillisV1,
    ) -> Result<Option<ClaimedJobV1>, StoreErrorV1> {
        (**self).claim_next(identity, worker, now)
    }

    fn cancel_before_claim(
        &self,
        identity: &DurableWorktreeIdentityV1,
        job: JobIdV1,
        expected_job_revision: RevisionV1,
        now: EpochMillisV1,
    ) -> Result<CancelOutcomeV1, StoreErrorV1> {
        (**self).cancel_before_claim(identity, job, expected_job_revision, now)
    }

    fn commit_terminal(
        &self,
        claim: ClaimTokenV1,
        expected_workspace_revision: RevisionV1,
        transition: Option<StateTransitionV1>,
        result: TerminalResultV1,
        now: EpochMillisV1,
    ) -> Result<TerminalReceiptV1, StoreErrorV1> {
        (**self).commit_terminal(claim, expected_workspace_revision, transition, result, now)
    }

    fn read_workspace_view(
        &self,
        identity: &DurableWorktreeIdentityV1,
    ) -> Result<WorkspaceViewV1, StoreErrorV1> {
        (**self).read_workspace_view(identity)
    }
}

impl<Store> StoreReadContractV1 for Arc<Store>
where
    Store: StoreReadContractV1 + ?Sized,
{
    fn read_session_aggregate(
        &self,
        identity: &DurableWorktreeIdentityV1,
    ) -> Result<Option<SessionAggregateV1>, StoreErrorV1> {
        (**self).read_session_aggregate(identity)
    }

    fn read_job(
        &self,
        identity: &DurableWorktreeIdentityV1,
        job: &JobIdV1,
    ) -> Result<Option<JobViewV1>, StoreErrorV1> {
        (**self).read_job(identity, job)
    }

    fn list_jobs(
        &self,
        identity: &DurableWorktreeIdentityV1,
        query: JobListQueryV1,
    ) -> Result<Vec<JobViewV1>, StoreErrorV1> {
        (**self).list_jobs(identity, query)
    }
}

impl<Store> StoreIdempotencyReadContractV1 for Arc<Store>
where
    Store: StoreIdempotencyReadContractV1 + ?Sized,
{
    fn read_idempotency_lookup(
        &self,
        identity: &DurableWorktreeIdentityV1,
        idempotency_key: &IdempotencyKeyV1,
    ) -> Result<Option<IdempotencyLookupV1>, StoreErrorV1> {
        (**self).read_idempotency_lookup(identity, idempotency_key)
    }

    fn read_idempotent_outcome(
        &self,
        identity: &DurableWorktreeIdentityV1,
        idempotency_key: &IdempotencyKeyV1,
        request_digest: &CanonicalRequestDigestV1,
    ) -> Result<Option<AdmitOutcomeV1>, StoreErrorV1> {
        (**self).read_idempotent_outcome(identity, idempotency_key, request_digest)
    }

    fn read_idempotent_execution(
        &self,
        identity: &DurableWorktreeIdentityV1,
        idempotency_key: &IdempotencyKeyV1,
    ) -> Result<Option<IdempotentExecutionV1>, StoreErrorV1> {
        (**self).read_idempotent_execution(identity, idempotency_key)
    }
}

impl<Store> StoreReconciliationReadContractV1 for Arc<Store>
where
    Store: StoreReconciliationReadContractV1 + ?Sized,
{
    fn read_reconciliation_snapshot(
        &self,
        identity: &DurableWorktreeIdentityV1,
        idempotency_key: &IdempotencyKeyV1,
    ) -> Result<ReconciliationSnapshotV1, StoreErrorV1> {
        (**self).read_reconciliation_snapshot(identity, idempotency_key)
    }
}

/// Exact durable workspace binding discovered without opening the store for mutation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceBindingV1 {
    identity: DurableWorktreeIdentityV1,
    last_validated_root: ValidatedWorkspaceRootV1,
}

impl WorkspaceBindingV1 {
    pub fn new(
        identity: DurableWorktreeIdentityV1,
        last_validated_root: ValidatedWorkspaceRootV1,
    ) -> Self {
        Self {
            identity,
            last_validated_root,
        }
    }

    pub fn identity(&self) -> &DurableWorktreeIdentityV1 {
        &self.identity
    }

    pub fn last_validated_root(&self) -> &ValidatedWorkspaceRootV1 {
        &self.last_validated_root
    }
}

/// Durable identity that every workspace operation must match before access.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DurableWorktreeIdentityV1 {
    common_dir_identity: GitIdentityV1,
    workspace_uuid: WorkspaceUuidV1,
    worktree_admin_identity: GitIdentityV1,
}

impl DurableWorktreeIdentityV1 {
    pub fn new(
        common_dir_identity: GitIdentityV1,
        workspace_uuid: WorkspaceUuidV1,
        worktree_admin_identity: GitIdentityV1,
    ) -> Self {
        Self {
            common_dir_identity,
            workspace_uuid,
            worktree_admin_identity,
        }
    }

    pub fn common_dir_identity(&self) -> &GitIdentityV1 {
        &self.common_dir_identity
    }

    pub fn workspace_uuid(&self) -> &WorkspaceUuidV1 {
        &self.workspace_uuid
    }

    pub fn worktree_admin_identity(&self) -> &GitIdentityV1 {
        &self.worktree_admin_identity
    }
}

/// A caller-provided key that binds retries to one canonical request digest.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IdempotencyKeyV1(String);

impl IdempotencyKeyV1 {
    pub fn new(value: impl Into<String>) -> Result<Self, StoreValueErrorV1> {
        let value = value.into();
        validate_non_empty_bounded(&value, MAX_IDEMPOTENCY_KEY_BYTES_V1, "idempotency key")?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Stable identifier of the scheduler worker that owns a claim.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkerIdV1(String);

impl WorkerIdV1 {
    pub fn new(value: impl Into<String>) -> Result<Self, StoreValueErrorV1> {
        let value = value.into();
        validate_non_empty_bounded(&value, MAX_WORKER_ID_BYTES_V1, "worker identifier")?;
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Claim ownership bound to one workspace, job revision, and worker.
///
/// Concrete SQLite claim code treats `job_revision` as a nonzero claim generation
/// derived from persisted `(job_id, request_digest, claimed_at_ms, worker)` data.
/// Recovery clears `claimed_at_ms`, so prior claim tokens cannot verify afterward.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClaimTokenV1 {
    identity: DurableWorktreeIdentityV1,
    job_id: JobIdV1,
    job_revision: RevisionV1,
    worker: WorkerIdV1,
}

impl ClaimTokenV1 {
    pub fn new(
        identity: DurableWorktreeIdentityV1,
        job_id: JobIdV1,
        job_revision: RevisionV1,
        worker: WorkerIdV1,
    ) -> Self {
        Self {
            identity,
            job_id,
            job_revision,
            worker,
        }
    }

    pub fn identity(&self) -> &DurableWorktreeIdentityV1 {
        &self.identity
    }

    pub fn job_id(&self) -> &JobIdV1 {
        &self.job_id
    }

    pub fn job_revision(&self) -> RevisionV1 {
        self.job_revision
    }

    pub fn worker(&self) -> &WorkerIdV1 {
        &self.worker
    }
}

/// Optimistic-concurrency conditions captured at admission and rechecked at commit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RevisionAttemptItemPreconditionsV1 {
    expected_session_revision: Option<RevisionV1>,
    expected_attempt_id: Option<AttemptId>,
    expected_item_id: Option<ItemId>,
    expected_item_revision: Option<RevisionV1>,
}

/// Session identity condition checked atomically when a new job is admitted.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AdmissionSessionIdentityV1 {
    Any,
    Absent,
    Exact(SessionId),
}

impl RevisionAttemptItemPreconditionsV1 {
    pub fn new(
        expected_session_revision: Option<RevisionV1>,
        expected_attempt_id: Option<AttemptId>,
        expected_item_id: Option<ItemId>,
        expected_item_revision: Option<RevisionV1>,
    ) -> Result<Self, StoreValueErrorV1> {
        if expected_item_revision.is_some() && expected_item_id.is_none() {
            return Err(StoreValueErrorV1::ItemRevisionWithoutItem);
        }
        Ok(Self {
            expected_session_revision,
            expected_attempt_id,
            expected_item_id,
            expected_item_revision,
        })
    }

    pub fn expected_session_revision(&self) -> Option<RevisionV1> {
        self.expected_session_revision
    }

    pub fn expected_attempt_id(&self) -> Option<&AttemptId> {
        self.expected_attempt_id.as_ref()
    }

    pub fn expected_item_id(&self) -> Option<&ItemId> {
        self.expected_item_id.as_ref()
    }

    pub fn expected_item_revision(&self) -> Option<RevisionV1> {
        self.expected_item_revision
    }
}

/// A fully canonicalized mutation awaiting durable admission.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdmitRequestV1 {
    admitted_procedure_snapshot: Option<Box<ProcedureSnapshotV1>>,
    canonical_execution: CanonicalExecutionJsonV1,
    command: CommandV1,
    execution_flavor: DurableExecutionFlavorV1,
    has_full_execution_document: bool,
    idempotency_key: IdempotencyKeyV1,
    job_id: JobIdV1,
    preconditions: RevisionAttemptItemPreconditionsV1,
    request_digest: CanonicalRequestDigestV1,
    response_context: Option<PersistedResponseContextV1>,
    submitted_at: EpochMillisV1,
    session_identity: AdmissionSessionIdentityV1,
}

impl AdmitRequestV1 {
    /// Builds a direct-Store request with a complete deterministic Store-command document.
    ///
    /// Protocol callers with a source request document must use
    /// [`Self::new_with_canonical_execution`] so recovery preserves that exact document.
    pub fn new(
        command: CommandV1,
        idempotency_key: IdempotencyKeyV1,
        job_id: JobIdV1,
        preconditions: RevisionAttemptItemPreconditionsV1,
        request_digest: CanonicalRequestDigestV1,
        submitted_at: EpochMillisV1,
    ) -> Self {
        Self {
            admitted_procedure_snapshot: None,
            canonical_execution: direct_store_canonical_execution_v1(&command, &preconditions),
            command,
            execution_flavor: DurableExecutionFlavorV1::LegacyV1,
            has_full_execution_document: true,
            idempotency_key,
            job_id,
            preconditions,
            request_digest,
            response_context: None,
            submitted_at,
            session_identity: AdmissionSessionIdentityV1::Any,
        }
    }

    /// Admits a command with its complete canonical execution document.
    pub fn new_with_canonical_execution(
        command: CommandV1,
        idempotency_key: IdempotencyKeyV1,
        job_id: JobIdV1,
        preconditions: RevisionAttemptItemPreconditionsV1,
        request_digest: CanonicalRequestDigestV1,
        submitted_at: EpochMillisV1,
        canonical_execution: CanonicalExecutionJsonV1,
    ) -> Self {
        Self {
            admitted_procedure_snapshot: None,
            canonical_execution,
            command,
            execution_flavor: DurableExecutionFlavorV1::LegacyV1,
            has_full_execution_document: true,
            idempotency_key,
            job_id,
            preconditions,
            request_digest,
            response_context: None,
            submitted_at,
            session_identity: AdmissionSessionIdentityV1::Any,
        }
    }

    pub fn with_session_identity(mut self, expected: AdmissionSessionIdentityV1) -> Self {
        self.session_identity = expected;
        self
    }

    /// Marks a complete execution document as a Procedure v2 runtime command.
    pub fn with_procedure_v2_execution(mut self) -> Self {
        self.execution_flavor = DurableExecutionFlavorV1::ProcedureV2;
        self
    }

    pub fn with_response_context(mut self, context: PersistedResponseContextV1) -> Self {
        self.response_context = Some(context);
        self
    }

    /// Binds the immutable Procedure snapshot that must be committed in the same transaction as
    /// this start job. The Store retains the normalized row independently of worker execution.
    pub fn with_admitted_procedure_snapshot(mut self, snapshot: ProcedureSnapshotV1) -> Self {
        self.admitted_procedure_snapshot = Some(Box::new(snapshot));
        self
    }

    pub fn admitted_procedure_snapshot(&self) -> Option<&ProcedureSnapshotV1> {
        self.admitted_procedure_snapshot.as_deref()
    }

    pub fn command(&self) -> &CommandV1 {
        &self.command
    }

    pub fn canonical_execution(&self) -> &CanonicalExecutionJsonV1 {
        &self.canonical_execution
    }

    pub const fn execution_flavor(&self) -> DurableExecutionFlavorV1 {
        self.execution_flavor
    }

    pub(crate) fn claimed_execution(&self) -> ClaimedExecutionV1 {
        ClaimedExecutionV1 {
            canonical_execution: self.canonical_execution.clone(),
            command: self.command.clone(),
            execution_flavor: self.execution_flavor,
            has_full_execution_document: self.has_full_execution_document,
            preconditions: self.preconditions.clone(),
            session_identity: self.session_identity.clone(),
        }
    }

    pub fn idempotency_key(&self) -> &IdempotencyKeyV1 {
        &self.idempotency_key
    }

    pub fn job_id(&self) -> &JobIdV1 {
        &self.job_id
    }

    pub fn preconditions(&self) -> &RevisionAttemptItemPreconditionsV1 {
        &self.preconditions
    }

    pub fn session_identity(&self) -> &AdmissionSessionIdentityV1 {
        &self.session_identity
    }

    pub fn request_digest(&self) -> &CanonicalRequestDigestV1 {
        &self.request_digest
    }

    pub fn response_context(&self) -> Option<&PersistedResponseContextV1> {
        self.response_context.as_ref()
    }

    pub fn submitted_at(&self) -> EpochMillisV1 {
        self.submitted_at
    }
}

/// Durable receipt for an admitted job.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JobReceiptV1 {
    identity_sequence: u64,
    job_id: JobIdV1,
    request_digest: CanonicalRequestDigestV1,
}

impl JobReceiptV1 {
    pub fn new(
        identity_sequence: u64,
        job_id: JobIdV1,
        request_digest: CanonicalRequestDigestV1,
    ) -> Self {
        Self {
            identity_sequence,
            job_id,
            request_digest,
        }
    }

    pub fn identity_sequence(&self) -> u64 {
        self.identity_sequence
    }

    pub fn job_id(&self) -> &JobIdV1 {
        &self.job_id
    }

    pub fn request_digest(&self) -> &CanonicalRequestDigestV1 {
        &self.request_digest
    }
}

/// A non-terminal job receipt or its immutable success, failure, or cancellation result.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum JobReceiptOrTerminalV1 {
    JobReceipt(JobReceiptV1),
    TerminalReceipt(PersistedTerminalReceiptV1),
}

/// Result of durable admission, distinguishing a new job from an idempotent replay.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AdmitOutcomeV1 {
    Existing(JobReceiptOrTerminalV1),
    New(JobReceiptV1),
}

/// Execution payload durably admitted with a job before a worker claims it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClaimedExecutionV1 {
    canonical_execution: CanonicalExecutionJsonV1,
    command: CommandV1,
    execution_flavor: DurableExecutionFlavorV1,
    has_full_execution_document: bool,
    preconditions: RevisionAttemptItemPreconditionsV1,
    session_identity: AdmissionSessionIdentityV1,
}

impl ClaimedExecutionV1 {
    /// Reconstructs a legacy command record that predates durable execution documents.
    pub fn new(command: CommandV1, preconditions: RevisionAttemptItemPreconditionsV1) -> Self {
        Self {
            canonical_execution: legacy_minimal_canonical_execution_v1(&command),
            command,
            execution_flavor: DurableExecutionFlavorV1::LegacyV1,
            has_full_execution_document: false,
            preconditions,
            session_identity: AdmissionSessionIdentityV1::Any,
        }
    }

    /// Reconstructs a claimed execution with its complete canonical document.
    pub fn new_with_canonical_execution(
        command: CommandV1,
        preconditions: RevisionAttemptItemPreconditionsV1,
        canonical_execution: CanonicalExecutionJsonV1,
    ) -> Self {
        Self {
            canonical_execution,
            command,
            execution_flavor: DurableExecutionFlavorV1::LegacyV1,
            has_full_execution_document: true,
            preconditions,
            session_identity: AdmissionSessionIdentityV1::Any,
        }
    }

    /// Reconstructs a claimed Procedure v2 execution with its complete immutable document.
    pub fn new_procedure_v2(
        command: CommandV1,
        preconditions: RevisionAttemptItemPreconditionsV1,
        canonical_execution: CanonicalExecutionJsonV1,
        session_identity: AdmissionSessionIdentityV1,
    ) -> Self {
        Self {
            canonical_execution,
            command,
            execution_flavor: DurableExecutionFlavorV1::ProcedureV2,
            has_full_execution_document: true,
            preconditions,
            session_identity,
        }
    }

    pub fn command(&self) -> &CommandV1 {
        &self.command
    }

    pub const fn execution_flavor(&self) -> DurableExecutionFlavorV1 {
        self.execution_flavor
    }

    pub fn session_identity(&self) -> &AdmissionSessionIdentityV1 {
        &self.session_identity
    }

    pub fn canonical_execution(&self) -> &CanonicalExecutionJsonV1 {
        &self.canonical_execution
    }

    /// Returns whether this execution has the complete canonical document needed after restart.
    ///
    /// `false` identifies a legacy v1 command record reconstructed from its minimal fields.
    pub fn has_complete_execution_document(&self) -> bool {
        self.has_full_execution_document
    }

    pub fn preconditions(&self) -> &RevisionAttemptItemPreconditionsV1 {
        &self.preconditions
    }
}

/// A running job claimed by one worker in FIFO order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClaimedJobV1 {
    claim: ClaimTokenV1,
    job: JobReceiptV1,
    execution: ClaimedExecutionV1,
    current_session: Option<SessionAggregateV1>,
}

impl ClaimedJobV1 {
    pub fn new_persisted(
        claim: ClaimTokenV1,
        job: JobReceiptV1,
        execution: ClaimedExecutionV1,
        current_session: Option<SessionAggregateV1>,
    ) -> Self {
        Self {
            claim,
            job,
            execution,
            current_session,
        }
    }

    pub fn claim(&self) -> &ClaimTokenV1 {
        &self.claim
    }

    pub fn job(&self) -> &JobReceiptV1 {
        &self.job
    }

    pub fn execution(&self) -> &ClaimedExecutionV1 {
        &self.execution
    }

    pub fn current_session(&self) -> Option<&SessionAggregateV1> {
        self.current_session.as_ref()
    }
}

/// The session aggregate mutation a successful terminal transaction must persist.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PersistedSessionMutationV1 {
    /// The transition has no session-row change.
    Unchanged,
    /// Replace the current normalized session state with this validated aggregate.
    Replace(SessionAggregateV1),
    /// Replace an existing session with a distinct fresh session at revision one.
    ReplaceFresh(SessionAggregateV1),
    /// Remove the current session and expose workspace revision zero.
    Clear,
}

/// Domain state transition committed together with a successful terminal result.
///
/// Store v1 defines workspace revision as the current session revision, or zero
/// when there is no current session. A concrete store must preserve that invariant
/// when applying a persisted session mutation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StateTransitionV1 {
    session_id: Option<SessionId>,
    previous_workspace_revision: RevisionV1,
    resulting_workspace_revision: RevisionV1,
    persisted_session_mutation: PersistedSessionMutationV1,
}

impl StateTransitionV1 {
    pub fn new_persisted(
        session_id: Option<SessionId>,
        previous_workspace_revision: RevisionV1,
        resulting_workspace_revision: RevisionV1,
        persisted_session_mutation: PersistedSessionMutationV1,
    ) -> Result<Self, StoreValueErrorV1> {
        match &persisted_session_mutation {
            PersistedSessionMutationV1::Unchanged
                if resulting_workspace_revision != previous_workspace_revision =>
            {
                return Err(StoreValueErrorV1::SessionMutationRevisionMismatch);
            }
            PersistedSessionMutationV1::Unchanged => {}
            PersistedSessionMutationV1::Replace(aggregate) => {
                if session_id.as_ref() != Some(aggregate.session_id())
                    || resulting_workspace_revision != aggregate.revision()
                    || resulting_workspace_revision
                        != previous_workspace_revision
                            .checked_next()
                            .map_err(|_| StoreValueErrorV1::RevisionRegressed)?
                {
                    return Err(StoreValueErrorV1::SessionMutationRevisionMismatch);
                }
            }
            PersistedSessionMutationV1::ReplaceFresh(aggregate) => {
                if session_id.as_ref() != Some(aggregate.session_id())
                    || resulting_workspace_revision != RevisionV1::new(1)
                    || aggregate.revision() != RevisionV1::new(1)
                {
                    return Err(StoreValueErrorV1::SessionMutationRevisionMismatch);
                }
            }
            PersistedSessionMutationV1::Clear => {
                if session_id.is_some() || resulting_workspace_revision != RevisionV1::ZERO {
                    return Err(StoreValueErrorV1::SessionMutationRevisionMismatch);
                }
            }
        }
        Ok(Self {
            session_id,
            previous_workspace_revision,
            resulting_workspace_revision,
            persisted_session_mutation,
        })
    }

    pub fn session_id(&self) -> Option<&SessionId> {
        self.session_id.as_ref()
    }

    pub fn previous_workspace_revision(&self) -> RevisionV1 {
        self.previous_workspace_revision
    }

    pub fn resulting_workspace_revision(&self) -> RevisionV1 {
        self.resulting_workspace_revision
    }

    pub fn persisted_session_mutation(&self) -> &PersistedSessionMutationV1 {
        &self.persisted_session_mutation
    }
}

/// Domain-only terminal payload persisted with a claimed job.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TerminalResultV1 {
    Failure(DomainError),
    Success(DomainResult),
}

/// Durable terminal result returned only after the state transaction commits.
///
/// A receipt intentionally stores no transition: failed terminal results therefore cannot
/// represent a state transition through this public type.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TerminalReceiptV1 {
    job: JobReceiptV1,
    result: TerminalResultV1,
}

impl TerminalReceiptV1 {
    pub fn new(job: JobReceiptV1, result: TerminalResultV1) -> Self {
        Self { job, result }
    }

    pub fn job(&self) -> &JobReceiptV1 {
        &self.job
    }

    pub fn result(&self) -> &TerminalResultV1 {
        &self.result
    }
}

/// Result of cancellation before a claim can start execution.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CancelOutcomeV1 {
    AlreadyTerminal(JobReceiptOrTerminalV1),
    Cancelled(JobReceiptV1),
}

/// Coherent read model for one workspace at a committed point in time.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorkspaceViewV1 {
    current_session: Option<SessionAggregateV1>,
    identity: DurableWorktreeIdentityV1,
    latest_workspace_sequence: u64,
    state: WorkspaceState,
    queued_job_count: u32,
    running_job_id: Option<JobIdV1>,
    observed_at: EpochMillisV1,
}

impl WorkspaceViewV1 {
    pub fn new(
        identity: DurableWorktreeIdentityV1,
        state: WorkspaceState,
        queued_job_count: u32,
        running_job_id: Option<JobIdV1>,
        observed_at: EpochMillisV1,
    ) -> Self {
        Self::new_coherent(
            identity,
            state,
            None,
            queued_job_count,
            running_job_id,
            0,
            observed_at,
        )
    }

    /// Builds a workspace view from facts observed in one coherent read transaction.
    pub fn new_coherent(
        identity: DurableWorktreeIdentityV1,
        state: WorkspaceState,
        current_session: Option<SessionAggregateV1>,
        queued_job_count: u32,
        running_job_id: Option<JobIdV1>,
        latest_workspace_sequence: u64,
        observed_at: EpochMillisV1,
    ) -> Self {
        Self {
            current_session,
            identity,
            latest_workspace_sequence,
            state,
            queued_job_count,
            running_job_id,
            observed_at,
        }
    }

    pub fn identity(&self) -> &DurableWorktreeIdentityV1 {
        &self.identity
    }

    pub fn current_session(&self) -> Option<&SessionAggregateV1> {
        self.current_session.as_ref()
    }

    pub fn state(&self) -> &WorkspaceState {
        &self.state
    }

    pub fn queued_job_count(&self) -> u32 {
        self.queued_job_count
    }

    pub fn running_job_id(&self) -> Option<&JobIdV1> {
        self.running_job_id.as_ref()
    }

    pub fn latest_workspace_sequence(&self) -> u64 {
        self.latest_workspace_sequence
    }

    pub fn observed_at(&self) -> EpochMillisV1 {
        self.observed_at
    }
}

/// Typed durable state of a queued, running, or terminal job.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JobStateV1 {
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

/// One coherent decoded job record.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JobViewV1 {
    execution: ClaimedExecutionV1,
    job: JobReceiptV1,
    state: JobStateV1,
    submitted_at: EpochMillisV1,
    claimed_at: Option<EpochMillisV1>,
    finished_at: Option<EpochMillisV1>,
    terminal_receipt: Option<PersistedTerminalReceiptV1>,
}

impl JobViewV1 {
    pub fn new(
        execution: ClaimedExecutionV1,
        job: JobReceiptV1,
        state: JobStateV1,
        submitted_at: EpochMillisV1,
        claimed_at: Option<EpochMillisV1>,
        finished_at: Option<EpochMillisV1>,
        terminal_receipt: Option<PersistedTerminalReceiptV1>,
    ) -> Self {
        Self {
            execution,
            job,
            state,
            submitted_at,
            claimed_at,
            finished_at,
            terminal_receipt,
        }
    }

    pub fn execution(&self) -> &ClaimedExecutionV1 {
        &self.execution
    }

    pub fn job(&self) -> &JobReceiptV1 {
        &self.job
    }

    pub fn state(&self) -> JobStateV1 {
        self.state
    }

    pub fn submitted_at(&self) -> EpochMillisV1 {
        self.submitted_at
    }

    pub fn claimed_at(&self) -> Option<EpochMillisV1> {
        self.claimed_at
    }

    pub fn finished_at(&self) -> Option<EpochMillisV1> {
        self.finished_at
    }

    pub fn terminal_receipt(&self) -> Option<&PersistedTerminalReceiptV1> {
        self.terminal_receipt.as_ref()
    }
}

/// Bounded, sequence-ordered query for durable job history.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct JobListQueryV1 {
    limit: u32,
}

impl JobListQueryV1 {
    pub fn new(limit: u32) -> Result<Self, StoreValueErrorV1> {
        if limit == 0 {
            return Err(StoreValueErrorV1::ZeroValue {
                field: "job list limit",
            });
        }
        if limit > MAX_JOB_LIST_LIMIT_V1 {
            return Err(StoreValueErrorV1::JobListLimitOutOfRange {
                maximum: MAX_JOB_LIST_LIMIT_V1,
            });
        }
        Ok(Self { limit })
    }

    pub fn limit(&self) -> u32 {
        self.limit
    }
}

/// Invalid input rejected before it reaches durable storage.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StoreValueErrorV1 {
    EmptyValue {
        field: &'static str,
    },
    ValueTooLong {
        field: &'static str,
        maximum_bytes: usize,
    },
    ItemRevisionWithoutItem,
    RevisionRegressed,
    ZeroValue {
        field: &'static str,
    },
    BusyTimeoutOutOfRange {
        maximum_ms: u32,
    },
    IntegerOutOfRange {
        field: &'static str,
    },
    InvalidWorkspaceRootEncoding,
    UnsupportedWorkspaceRootPlatform,
    UnsupportedPrivatePermissionPlatform,
    SessionMutationRevisionMismatch,
    InvalidCanonicalExecutionJson,
    CanonicalExecutionJsonDepthExceeded {
        maximum: usize,
    },
    JobListLimitOutOfRange {
        maximum: u32,
    },
    InvalidProcedureV2State {
        reason: &'static str,
    },
}

/// Typed storage failures. None of these variants is a public protocol envelope.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StoreErrorV1 {
    /// Admission rows committed, but a later acknowledgement step failed.
    AdmissionCommittedV1 {
        receipt: JobReceiptV1,
        source: Box<StoreErrorV1>,
    },
    /// SQLite could not prove whether the admission commit became durable.
    AdmissionOutcomeUnknownV1 {
        idempotency_key: IdempotencyKeyV1,
    },
    AlreadyClaimedV1 {
        job_id: JobIdV1,
    },
    CancellationLostV1 {
        job_id: JobIdV1,
    },
    ClaimStaleV1 {
        job_id: JobIdV1,
    },
    CorruptStateV1 {
        record: StoreRecordKindV1,
    },
    IdempotencyDigestConflictV1 {
        expected: CanonicalRequestDigestV1,
        actual: CanonicalRequestDigestV1,
    },
    InternalInvariantViolationV1 {
        invariant: StoreInvariantV1,
    },
    InvalidStateV1(StoreValueErrorV1),
    PrimaryOperationAndCleanupFailureV1 {
        primary: Box<StoreErrorV1>,
        cleanup: Box<StoreErrorV1>,
    },
    JobNotFoundV1 {
        job_id: JobIdV1,
    },
    NewerStateV1 {
        found_schema_version: u32,
        supported_schema_version: u32,
    },
    PreconditionConflictV1 {
        expected: Option<RevisionV1>,
        actual: Option<RevisionV1>,
    },
    ProcedureV2PreconditionFailedV1 {
        failure: PersistedGraphMutationFailureV2,
    },
    SessionIdentityConflictV1 {
        expected: Option<SessionId>,
        actual: Option<SessionId>,
    },
    StorageIntegrityV1 {
        check: StoreIntegrityCheckV1,
    },
    StorageUnavailableV1 {
        reason: StoreUnavailableReasonV1,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StoreRecordKindV1 {
    Workspace,
    Job,
    IdempotencyRecord,
    Session,
    Snapshot,
    Migration,
    Attempt,
    Item,
    Blocker,
    Journal,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StoreUnavailableReasonV1 {
    Busy,
    Locked,
    Recovery,
    StorageIo,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StoreIntegrityCheckV1 {
    WorkspaceIdentity,
    SnapshotDigest,
    SessionCursor,
    ActiveAttempt,
    JobQueue,
    SchemaVersion,
    MigrationChecksum,
    RequiredSchemaObjects,
    ConnectionPragmas,
    SqliteQuickCheck,
    SqliteDeepCheck,
    ForeignKeys,
    InternalCodec,
    IdempotencyReceipt,
    ClaimOwnership,
}
#[derive(Clone, Debug)]
pub(crate) enum RusqliteErrorContextV1 {
    Integrity(StoreIntegrityCheckV1),
    Record(StoreRecordKindV1),
}

impl RusqliteErrorContextV1 {
    fn persisted_data_error(self) -> StoreErrorV1 {
        match self {
            Self::Integrity(check) => StoreErrorV1::StorageIntegrityV1 { check },
            Self::Record(record) => StoreErrorV1::CorruptStateV1 { record },
        }
    }
}

/// Maps a rusqlite failure at the caller's persisted-data boundary.
///
/// Busy, lock, and operating-system SQLite errors remain unavailable. SQLite corruption,
/// row-conversion, required-row, and query-shape failures are persisted-data failures and retain
/// the boundary supplied by the caller.
pub(crate) fn map_rusqlite_error_v1(
    error: rusqlite::Error,
    context: RusqliteErrorContextV1,
) -> StoreErrorV1 {
    match error {
        rusqlite::Error::SqliteFailure(failure, _) => match failure.code {
            rusqlite::ErrorCode::DatabaseBusy => StoreErrorV1::StorageUnavailableV1 {
                reason: StoreUnavailableReasonV1::Busy,
            },
            rusqlite::ErrorCode::DatabaseLocked => StoreErrorV1::StorageUnavailableV1 {
                reason: StoreUnavailableReasonV1::Locked,
            },
            rusqlite::ErrorCode::DatabaseCorrupt
            | rusqlite::ErrorCode::NotADatabase
            | rusqlite::ErrorCode::InternalMalfunction
            | rusqlite::ErrorCode::SchemaChanged
            | rusqlite::ErrorCode::ConstraintViolation
            | rusqlite::ErrorCode::TypeMismatch
            | rusqlite::ErrorCode::ApiMisuse
            | rusqlite::ErrorCode::ParameterOutOfRange => context.persisted_data_error(),
            _ => StoreErrorV1::StorageUnavailableV1 {
                reason: StoreUnavailableReasonV1::StorageIo,
            },
        },
        rusqlite::Error::FromSqlConversionFailure(..)
        | rusqlite::Error::IntegralValueOutOfRange(..)
        | rusqlite::Error::InvalidColumnType(..)
        | rusqlite::Error::Utf8Error(_)
        | rusqlite::Error::QueryReturnedNoRows
        | rusqlite::Error::InvalidColumnIndex(_)
        | rusqlite::Error::InvalidColumnName(_)
        | rusqlite::Error::InvalidParameterName(_)
        | rusqlite::Error::ExecuteReturnedResults
        | rusqlite::Error::StatementChangedRows(_)
        | rusqlite::Error::InvalidQuery => context.persisted_data_error(),
        _ => StoreErrorV1::StorageUnavailableV1 {
            reason: StoreUnavailableReasonV1::StorageIo,
        },
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum StoreInvariantV1 {
    OneCurrentSession,
    OneActiveAttempt,
    MonotonicWorkspaceSequence,
    MonotonicRevision,
    ClaimedJobMustBeRunning,
    QueueSequence,
    TransitionMutationShape,
    RecoveryParity,
    RetentionAccounting,
    ResetSeed,
    Publication,
    SchemaDefinition,
    ProcedureV2GraphState,
}

impl fmt::Display for StoreValueErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyValue { field } => write!(formatter, "{field} must not be empty"),
            Self::ValueTooLong {
                field,
                maximum_bytes,
            } => write!(formatter, "{field} exceeds {maximum_bytes} bytes"),
            Self::ItemRevisionWithoutItem => {
                formatter.write_str("an item revision requires an item identifier")
            }
            Self::RevisionRegressed => formatter.write_str("revision must not regress"),
            Self::ZeroValue { field } => write!(formatter, "{field} must be nonzero"),
            Self::BusyTimeoutOutOfRange { maximum_ms } => {
                write!(
                    formatter,
                    "busy timeout must be between 1 and {maximum_ms} ms"
                )
            }
            Self::IntegerOutOfRange { field } => {
                write!(formatter, "{field} exceeds SQLite's signed integer range")
            }
            Self::InvalidWorkspaceRootEncoding => {
                formatter.write_str("workspace root is not canonical lowercase hexadecimal")
            }
            Self::UnsupportedWorkspaceRootPlatform => {
                formatter.write_str("workspace root byte conversion requires a Unix platform")
            }
            Self::UnsupportedPrivatePermissionPlatform => {
                formatter.write_str("private file permission verification requires a Unix platform")
            }
            Self::SessionMutationRevisionMismatch => {
                formatter.write_str("persisted session mutation disagrees with workspace revision")
            }
            Self::InvalidCanonicalExecutionJson => {
                formatter.write_str("execution JSON is not exact canonical JSON")
            }
            Self::CanonicalExecutionJsonDepthExceeded { maximum } => {
                write!(formatter, "execution JSON nesting exceeds {maximum}")
            }
            Self::JobListLimitOutOfRange { maximum } => {
                write!(formatter, "job list limit must be between 1 and {maximum}")
            }
            Self::InvalidProcedureV2State { reason } => formatter.write_str(reason),
        }
    }
}

impl std::error::Error for StoreValueErrorV1 {}

impl fmt::Display for StoreErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

impl std::error::Error for StoreErrorV1 {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::AdmissionCommittedV1 { source, .. }
            | Self::PrimaryOperationAndCleanupFailureV1 {
                primary: source, ..
            } => Some(source.as_ref()),
            _ => None,
        }
    }
}

fn encode_lower_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(WORKSPACE_ROOT_TEXT_PREFIX_V1.len() + bytes.len() * 2);
    encoded.push_str(WORKSPACE_ROOT_TEXT_PREFIX_V1);
    for byte in bytes {
        encoded.push(char::from(HEX[usize::from(byte >> 4)]));
        encoded.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    encoded
}

fn decode_lower_hex(hex: &str) -> Result<Vec<u8>, StoreValueErrorV1> {
    if hex.is_empty() || (hex.len() & 1) != 0 {
        return Err(StoreValueErrorV1::InvalidWorkspaceRootEncoding);
    }

    let mut bytes = Vec::with_capacity(hex.len() / 2);
    for pair in hex.as_bytes().chunks_exact(2) {
        let high = decode_lower_hex_nibble(pair[0])?;
        let low = decode_lower_hex_nibble(pair[1])?;
        bytes.push((high << 4) | low);
    }
    Ok(bytes)
}

fn decode_lower_hex_nibble(byte: u8) -> Result<u8, StoreValueErrorV1> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        _ => Err(StoreValueErrorV1::InvalidWorkspaceRootEncoding),
    }
}
fn validate_non_empty_bounded(
    value: &str,
    maximum_bytes: usize,
    field: &'static str,
) -> Result<(), StoreValueErrorV1> {
    if value.is_empty() {
        return Err(StoreValueErrorV1::EmptyValue { field });
    }
    if value.len() > maximum_bytes {
        return Err(StoreValueErrorV1::ValueTooLong {
            field,
            maximum_bytes,
        });
    }
    Ok(())
}
