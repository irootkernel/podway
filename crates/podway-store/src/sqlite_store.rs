//! Synchronous single-writer SQLite implementation of Store v1.

use std::collections::hash_map::RandomState;
use std::fs::{self, File, OpenOptions};
use std::hash::{BuildHasher, Hash, Hasher};
use std::io::ErrorKind;
#[cfg(unix)]
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::{Connection, OptionalExtension, Row, Transaction, TransactionBehavior, params};

use crate::codec::{
    PersistedDomainResultV1, PersistedTerminalJobProjectionV1, PersistedTerminalJobStateV1,
    PersistedTerminalReceiptV1, PersistedTerminalResultV1, PersistedTerminalSessionProjectionV1,
    decode_command_v1, decode_terminal_receipt_v1, encode_command_v1,
    encode_persisted_terminal_receipt_v1, validate_persisted_terminal_result_for_command_v1,
    validate_terminal_result_for_command_v1,
};
use crate::schema::{
    DatabasePathStateV1, canonical_database_path_v1, inspect_database_path_v1,
    inspect_database_snapshot_unbound_v1, inspect_database_snapshot_v1, open_or_initialize_v1,
    open_or_initialize_with_temporary_cleanup_arm_v1, recover_interrupted_publication_v1,
    validate_database_parent_path_v1, validate_existing_database_path_v1,
    validate_existing_regular_private_file_metadata_v1, validate_existing_regular_private_file_v1,
    validate_publication_link_pair_v1, verify_inspection_integrity_connection_v1, verify_schema_v1,
    write_temporary_ownership_marker_v1,
};
use crate::state_rows::{
    load_current_session, load_workspace_state, persist_snapshot, replace_current_session,
};
use crate::{
    AdmitOutcomeV1, AdmitRequestV1, CancelOutcomeV1, CanonicalRequestDigestV1, ClaimTokenV1,
    ClaimedExecutionV1, ClaimedJobV1, DurableWorktreeIdentityV1, EpochMillisV1, IntegrityModeV1,
    JobIdV1, JobListQueryV1, JobReceiptOrTerminalV1, JobReceiptV1, JobStateV1, JobViewV1,
    PersistedSessionMutationV1, PruneReportV1, RecoveryReportV1, RevisionV1,
    RusqliteErrorContextV1, SqliteStoreOptionsV1, StateTransitionV1, StoreContractV1, StoreErrorV1,
    StoreFailpointV1, StoreIdempotencyReadContractV1, StoreIntegrityCheckV1, StoreInvariantV1,
    StoreReadContractV1, StoreRecordKindV1, StoreUnavailableReasonV1, TerminalReceiptV1,
    TerminalResultV1, ValidatedWorkspaceRootV1, WorkerIdV1, WorkspaceBindingV1, WorkspaceViewV1,
    command_is_session_scoped_v1, command_name_v1, map_rusqlite_error_v1,
};

/// One process-local, synchronous SQLite v1 workspace store.
pub struct SqliteStoreV1 {
    connection: Mutex<Connection>,
    database_path: PathBuf,
    identity: DurableWorktreeIdentityV1,
    options: SqliteStoreOptionsV1,
    startup_recovery_report: RecoveryReportV1,
    claim_hasher: RandomState,
}

impl SqliteStoreV1 {
    /// Opens an existing v1 database or atomically installs a fully initialized new one.
    pub fn open(
        path: impl AsRef<Path>,
        root: &ValidatedWorkspaceRootV1,
        identity: DurableWorktreeIdentityV1,
        options: SqliteStoreOptionsV1,
        now: EpochMillisV1,
    ) -> Result<Self, StoreErrorV1> {
        let database_path = canonical_database_path_v1(path.as_ref())?;
        recover_interrupted_publication_v1(&database_path)?;
        let mut connection = match inspect_database_path_v1(&database_path)? {
            DatabasePathStateV1::Existing => {
                open_or_initialize_v1(&database_path, root, &identity, &options, now)?
            }
            DatabasePathStateV1::Missing => {
                initialize_new_database_atomically(&database_path, root, &identity, &options, now)?
            }
        };
        crate::schema::verify_integrity_connection_v1(
            &mut connection,
            &identity,
            &options,
            IntegrityModeV1::Fast,
            now,
        )?;
        let startup_recovery_report = recover_running_connection(&mut connection, &options, now)?;
        Ok(Self {
            connection: Mutex::new(connection),
            database_path,
            identity,
            options,
            startup_recovery_report,
            claim_hasher: RandomState::new(),
        })
    }

    /// Reads a validated existing workspace binding without creating or modifying durable state.
    pub fn inspect_workspace_binding(
        path: impl AsRef<Path>,
        options: &SqliteStoreOptionsV1,
    ) -> Result<Option<WorkspaceBindingV1>, StoreErrorV1> {
        let database_path = canonical_database_path_v1(path.as_ref())?;
        match inspect_database_path_v1(&database_path)? {
            DatabasePathStateV1::Missing => return Ok(None),
            DatabasePathStateV1::Existing => {}
        }

        // The preflight rejects interrupted publication links and the unbound snapshot helper
        // never invokes recovery, so this inspection cannot finalize a publish.

        let binding =
            inspect_database_snapshot_unbound_v1(&database_path, options, |connection| {
                verify_schema_v1(connection)?;
                let (
                    workspace_uuid,
                    common_dir_identity,
                    worktree_admin_identity,
                    last_validated_root,
                ): (String, String, String, String) = connection
                    .query_row(
                        "SELECT workspace_uuid, git_common_fingerprint, git_worktree_fingerprint, \
                         last_validated_root FROM workspace_state WHERE singleton = 1",
                        [],
                        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
                    )
                    .map_err(|error| storage_record(error, StoreRecordKindV1::Workspace))?;
                let identity = DurableWorktreeIdentityV1::new(
                    CanonicalRequestDigestV1::new(common_dir_identity)
                        .map_err(|_| corrupt(StoreRecordKindV1::Workspace))?,
                    crate::WorkspaceUuidV1::new(workspace_uuid)
                        .map_err(|_| corrupt(StoreRecordKindV1::Workspace))?,
                    CanonicalRequestDigestV1::new(worktree_admin_identity)
                        .map_err(|_| corrupt(StoreRecordKindV1::Workspace))?,
                );
                let last_validated_root =
                    ValidatedWorkspaceRootV1::from_encoded(last_validated_root)
                        .map_err(|_| corrupt(StoreRecordKindV1::Workspace))?;
                verify_inspection_integrity_connection_v1(
                    connection,
                    &identity,
                    options,
                    IntegrityModeV1::Fast,
                    EpochMillisV1::new(0),
                )?;
                Ok(WorkspaceBindingV1::new(identity, last_validated_root))
            })?;
        Ok(Some(binding))
    }

    /// Checkpoints and closes the sole SQLite connection before daemon-owned maintenance.
    pub fn close_for_maintenance(self) -> Result<(), StoreErrorV1> {
        let database_path = self.database_path;
        let connection =
            self.connection
                .into_inner()
                .map_err(|_| StoreErrorV1::StorageUnavailableV1 {
                    reason: StoreUnavailableReasonV1::Recovery,
                })?;
        checkpoint_wal_truncate(&connection)?;
        connection.close().map_err(|(_, error)| storage(error))?;
        verify_checkpointed_wal_is_empty(&database_path)
    }

    /// Seeds a new reset target or verifies a target published by an earlier identical attempt.
    pub fn seed_or_verify_reset_target(
        path: impl AsRef<Path>,
        root: &ValidatedWorkspaceRootV1,
        target_identity: DurableWorktreeIdentityV1,
        options: SqliteStoreOptionsV1,
        request: AdmitRequestV1,
        result: podway_core::DomainResult,
        now: EpochMillisV1,
    ) -> Result<TerminalReceiptV1, StoreErrorV1> {
        validate_reset_seed_input(&target_identity, &request, &result)?;
        let receipt = reset_terminal_receipt(&request, result);
        let canonical_request = encode_command_v1(&request.claimed_execution())
            .map_err(|_| corrupt(StoreRecordKindV1::Job))?;
        let terminal = PersistedTerminalReceiptV1::new_with_projections(
            receipt.job().clone(),
            PersistedTerminalResultV1::from_terminal_result(receipt.result()),
            PersistedTerminalJobProjectionV1::new(
                PersistedTerminalJobStateV1::Succeeded,
                request.submitted_at(),
                None,
                now,
            )
            .map_err(|_| corrupt(StoreRecordKindV1::Job))?,
            None,
        )
        .map_err(|_| corrupt(StoreRecordKindV1::Job))?;
        let terminal_response = encode_persisted_terminal_receipt_v1(&terminal)
            .map_err(|_| corrupt(StoreRecordKindV1::Job))?;
        let context = ResetSeedContextV1 {
            root,
            identity: &target_identity,
            options: &options,
            request: &request,
            receipt: &receipt,
            canonical_request: &canonical_request,
            terminal_response: &terminal_response,
            now,
        };
        let path = canonical_database_path_v1(path.as_ref())?;
        recover_interrupted_publication_v1(&path)?;
        match inspect_database_path_v1(&path)? {
            DatabasePathStateV1::Existing => return verify_seeded_reset_target(&path, &context),
            DatabasePathStateV1::Missing => {}
        }

        let temporary = create_temporary_database(&path, now)?;
        let mut cleanup_armed = options.failpoint()
            == Some(StoreFailpointV1::ResetAfterPublicationBeforeResponseAndTemporaryCleanup);
        let seeded = seed_reset_target(
            &temporary.database,
            &temporary._database_file,
            &path,
            &context,
            &mut cleanup_armed,
        );
        let cleanup = cleanup_temporary_database(&temporary, cleanup_armed);
        let seeded = combine_operation_and_cleanup(seeded, cleanup)?;
        match seeded {
            PublicationOutcomeV1::Published => {
                options.trigger_failpoint(StoreFailpointV1::ResetAfterPublicationBeforeResponse)?;
                Ok(receipt)
            }
            PublicationOutcomeV1::Existing => verify_seeded_reset_target(&path, &context),
        }
    }

    /// Reports the one-time recovery completed before this store was published.
    pub fn startup_recovery_report(&self) -> &RecoveryReportV1 {
        &self.startup_recovery_report
    }

    /// Prunes bounded terminal history after checking the bound workspace identity.
    pub fn prune_terminal_history(
        &self,
        identity: &DurableWorktreeIdentityV1,
        now: EpochMillisV1,
    ) -> Result<PruneReportV1, StoreErrorV1> {
        self.require_identity(identity)?;
        let mut connection = self.lock_connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        let report = prune_terminal_history_transaction(&transaction, now, &self.options, false)?;
        transaction.commit().map_err(storage)?;
        Ok(report)
    }

    /// Loads the complete validated normalized session aggregate for daemon diagnostics.
    pub fn read_session_aggregate(
        &self,
        identity: &DurableWorktreeIdentityV1,
    ) -> Result<Option<podway_core::SessionAggregateV1>, StoreErrorV1> {
        self.require_identity(identity)?;
        let mut connection = self.lock_connection()?;
        let transaction = connection.transaction().map_err(storage)?;
        let aggregate = load_current_session(&transaction)?;
        transaction.commit().map_err(storage)?;
        Ok(aggregate)
    }

    fn require_identity(&self, identity: &DurableWorktreeIdentityV1) -> Result<(), StoreErrorV1> {
        if self.identity != *identity {
            return Err(StoreErrorV1::StorageIntegrityV1 {
                check: StoreIntegrityCheckV1::WorkspaceIdentity,
            });
        }
        Ok(())
    }

    fn lock_connection(&self) -> Result<std::sync::MutexGuard<'_, Connection>, StoreErrorV1> {
        self.connection
            .lock()
            .map_err(|_| StoreErrorV1::StorageUnavailableV1 {
                reason: StoreUnavailableReasonV1::Recovery,
            })
    }

    fn claim_generation(
        &self,
        identity: &DurableWorktreeIdentityV1,
        job: &JobIdV1,
        digest: &CanonicalRequestDigestV1,
        claimed_at: EpochMillisV1,
        worker: &WorkerIdV1,
    ) -> RevisionV1 {
        let mut hasher = self.claim_hasher.build_hasher();
        identity.workspace_uuid().as_str().hash(&mut hasher);
        identity.common_dir_identity().as_str().hash(&mut hasher);
        identity
            .worktree_admin_identity()
            .as_str()
            .hash(&mut hasher);
        job.as_str().hash(&mut hasher);
        digest.as_str().hash(&mut hasher);
        claimed_at.get().hash(&mut hasher);
        worker.as_str().hash(&mut hasher);
        let generation = hasher.finish().max(1);
        RevisionV1::new(generation)
    }
    fn trigger_failpoint(&self, failpoint: StoreFailpointV1) -> Result<(), StoreErrorV1> {
        self.options.trigger_failpoint(failpoint)
    }
}

impl StoreContractV1 for SqliteStoreV1 {
    fn admit(
        &self,
        identity: &DurableWorktreeIdentityV1,
        request: AdmitRequestV1,
    ) -> Result<AdmitOutcomeV1, StoreErrorV1> {
        self.require_identity(identity)?;
        if matches!(request.command(), crate::CommandV1::WorkspaceResetAll) {
            return Err(invariant(StoreInvariantV1::TransitionMutationShape));
        }
        if request.admitted_procedure_snapshot().is_some()
            && !matches!(
                request.command(),
                crate::CommandV1::SessionStart | crate::CommandV1::SessionStartReplace
            )
        {
            return Err(invariant(StoreInvariantV1::TransitionMutationShape));
        }
        self.trigger_failpoint(StoreFailpointV1::AdmissionBeforeTransaction)?;
        let mut connection = self.lock_connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        let existing: Option<(String, String, Option<String>)> = transaction
            .query_row(
                "SELECT request_digest, job_id, terminal_response_json FROM idempotency_records \
                 WHERE idempotency_key = ?1",
                [request.idempotency_key().as_str()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(|error| storage_record(error, StoreRecordKindV1::IdempotencyRecord))?;
        if let Some((stored_digest, job_id, terminal)) = existing {
            let stored_digest = digest(stored_digest)?;
            if stored_digest != *request.request_digest() {
                return Err(StoreErrorV1::IdempotencyDigestConflictV1 {
                    expected: stored_digest,
                    actual: request.request_digest().clone(),
                });
            }
            let outcome = replay_for_idempotency(&transaction, &job_id, &stored_digest, terminal)?;
            transaction.commit().map_err(storage)?;
            return Ok(AdmitOutcomeV1::Existing(outcome));
        }
        let current_session = load_current_session(&transaction)?;
        let actual_session_id = current_session
            .as_ref()
            .map(podway_core::SessionAggregateV1::session_id)
            .cloned();
        let session_identity_matches = match request.session_identity() {
            crate::AdmissionSessionIdentityV1::Any => true,
            crate::AdmissionSessionIdentityV1::Absent => actual_session_id.is_none(),
            crate::AdmissionSessionIdentityV1::Exact(expected) => {
                actual_session_id.as_ref() == Some(expected)
            }
        };
        if !session_identity_matches {
            let expected = match request.session_identity() {
                crate::AdmissionSessionIdentityV1::Any
                | crate::AdmissionSessionIdentityV1::Absent => None,
                crate::AdmissionSessionIdentityV1::Exact(expected) => Some(expected.clone()),
            };
            return Err(StoreErrorV1::SessionIdentityConflictV1 {
                expected,
                actual: actual_session_id,
            });
        }
        let barrier_exists: i64 = transaction
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM jobs WHERE state IN ('queued', 'running') \
                 AND command_name IN ('workspace.reset_all', 'session.reset', 'session.start_replace'))",
                [],
                |row| row.get(0),
            )
            .map_err(storage)?;
        if barrier_exists != 0 {
            return Err(StoreErrorV1::StorageUnavailableV1 {
                reason: StoreUnavailableReasonV1::Busy,
            });
        }
        let scope_session_id = admission_session_scope(
            &transaction,
            request.command(),
            request.preconditions().expected_session_revision(),
        )?;
        let scope_kind = if command_is_session_scoped_v1(request.command()) {
            "session"
        } else {
            "workspace"
        };
        let pending: i64 = transaction
            .query_row(
                "SELECT COUNT(*) FROM jobs WHERE state = 'queued'",
                [],
                |row| row.get(0),
            )
            .map_err(storage)?;
        if pending >= i64::from(self.options.max_pending_jobs()) {
            return Err(StoreErrorV1::StorageUnavailableV1 {
                reason: StoreUnavailableReasonV1::Busy,
            });
        }
        if let Some(snapshot) = request.admitted_procedure_snapshot() {
            persist_snapshot(&transaction, snapshot)?;
        }
        let sequence: i64 = transaction
            .query_row(
                "UPDATE workspace_state SET next_workspace_sequence = next_workspace_sequence + 1, \
                 updated_at_ms = ?1 WHERE singleton = 1 RETURNING next_workspace_sequence",
                [sqlite_u64(request.submitted_at().get())?],
                |row| row.get(0),
            )
            .map_err(storage)?;
        let sequence =
            u64::try_from(sequence).map_err(|_| invariant(StoreInvariantV1::QueueSequence))?;
        let execution = request.claimed_execution();
        let canonical_request =
            encode_command_v1(&execution).map_err(|_| corrupt(StoreRecordKindV1::Job))?;
        let inserted_job = transaction
            .execute(
                "INSERT INTO jobs (job_id, workspace_sequence, idempotency_key, request_digest, command_name, \
                 canonical_request_json, state, session_id, submitted_at_ms) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'queued', ?7, ?8)",
                params![
                    request.job_id().as_str(),
                    sqlite_u64(sequence)?,
                    request.idempotency_key().as_str(),
                    request.request_digest().as_str(),
                    command_name_v1(request.command()),
                    canonical_request,
                    scope_session_id.as_deref(),
                    sqlite_u64(request.submitted_at().get())?,
                ],
            )
            .map_err(storage)?;
        if inserted_job != 1 {
            return Err(invariant(StoreInvariantV1::QueueSequence));
        }
        let inserted_idempotency = transaction
            .execute(
                "INSERT INTO idempotency_records (idempotency_key, request_digest, job_id, scope_kind, \
                 scope_session_id, created_at_ms, updated_at_ms) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
                params![
                    request.idempotency_key().as_str(),
                    request.request_digest().as_str(),
                    request.job_id().as_str(),
                    scope_kind,
                    scope_session_id.as_deref(),
                    sqlite_u64(request.submitted_at().get())?,
                ],
            )
            .map_err(storage)?;
        if inserted_idempotency != 1 {
            return Err(invariant(StoreInvariantV1::QueueSequence));
        }
        let receipt = JobReceiptV1::new(
            sequence,
            request.job_id().clone(),
            request.request_digest().clone(),
        );
        self.trigger_failpoint(StoreFailpointV1::AdmissionAfterDurableRowsBeforeCommit)?;
        transaction.commit().map_err(storage)?;
        self.trigger_failpoint(StoreFailpointV1::AdmissionAfterCommit)?;
        Ok(AdmitOutcomeV1::New(receipt))
    }

    fn claim_next(
        &self,
        identity: &DurableWorktreeIdentityV1,
        worker: WorkerIdV1,
        now: EpochMillisV1,
    ) -> Result<Option<ClaimedJobV1>, StoreErrorV1> {
        self.require_identity(identity)?;
        let mut connection = self.lock_connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        let running_exists: i64 = transaction
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM jobs WHERE state = 'running')",
                [],
                |row| row.get(0),
            )
            .map_err(storage)?;
        if running_exists != 0 {
            transaction.commit().map_err(storage)?;
            return Ok(None);
        }
        let job: Option<JobRow> = transaction
            .query_row(
                "SELECT job_id, workspace_sequence, request_digest, command_name, canonical_request_json FROM jobs \
                 WHERE state = 'queued' ORDER BY workspace_sequence LIMIT 1",
                [],
                |row| Ok(JobRow {
                    job_id: row.get(0)?,
                    sequence: row.get(1)?,
                    digest: row.get(2)?,
                    command_name: row.get(3)?,
                    request: row.get(4)?,
                }),
            )
            .optional()
            .map_err(|error| storage_record(error, StoreRecordKindV1::Job))?;
        let Some(job) = job else {
            transaction.commit().map_err(storage)?;
            return Ok(None);
        };
        let claimed_at = sqlite_u64(now.get())?;
        let changed = transaction
            .execute(
                "UPDATE jobs SET state = 'running', claimed_at_ms = ?1 WHERE job_id = ?2 AND state = 'queued'",
                params![claimed_at, job.job_id.as_str()],
            )
            .map_err(storage)?;
        if changed != 1 {
            return Err(invariant(StoreInvariantV1::QueueSequence));
        }
        let job_id = job_id(job.job_id)?;
        let digest = digest(job.digest)?;
        verify_persisted_job_scope(&transaction, job_id.as_str())?;
        let sequence =
            u64::try_from(job.sequence).map_err(|_| invariant(StoreInvariantV1::QueueSequence))?;
        if sequence == 0 {
            return Err(corrupt(StoreRecordKindV1::Job));
        }
        let execution =
            decode_command_v1(&job.request).map_err(|_| corrupt(StoreRecordKindV1::Job))?;
        if command_name_v1(execution.command()) != job.command_name {
            return Err(corrupt(StoreRecordKindV1::Job));
        }
        let session = load_current_session(&transaction)?;
        let claim = ClaimTokenV1::new(
            self.identity.clone(),
            job_id.clone(),
            self.claim_generation(&self.identity, &job_id, &digest, now, &worker),
            worker,
        );
        let result = ClaimedJobV1::new_persisted(
            claim,
            JobReceiptV1::new(sequence, job_id, digest),
            execution,
            session,
        );
        transaction.commit().map_err(storage)?;
        self.trigger_failpoint(StoreFailpointV1::ClaimAfterCommit)?;
        Ok(Some(result))
    }

    fn cancel_before_claim(
        &self,
        identity: &DurableWorktreeIdentityV1,
        job: JobIdV1,
        expected_job_revision: RevisionV1,
        now: EpochMillisV1,
    ) -> Result<CancelOutcomeV1, StoreErrorV1> {
        self.require_identity(identity)?;
        let mut connection = self.lock_connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        let row: Option<JobStateRow> = transaction
            .query_row(
                "SELECT workspace_sequence, request_digest, state, terminal_response_json, \
                 submitted_at_ms, claimed_at_ms, finished_at_ms FROM jobs WHERE job_id = ?1",
                [job.as_str()],
                |row| {
                    Ok(JobStateRow {
                        sequence: row.get(0)?,
                        digest: row.get(1)?,
                        state: row.get(2)?,
                        terminal: row.get(3)?,
                        submitted_at: row.get(4)?,
                        claimed_at: row.get(5)?,
                        finished_at: row.get(6)?,
                    })
                },
            )
            .optional()
            .map_err(|error| storage_record(error, StoreRecordKindV1::Job))?;
        let Some(row) = row else {
            return Err(StoreErrorV1::JobNotFoundV1 { job_id: job });
        };
        let sequence =
            u64::try_from(row.sequence).map_err(|_| invariant(StoreInvariantV1::QueueSequence))?;
        if sequence == 0 {
            return Err(corrupt(StoreRecordKindV1::Job));
        }
        let digest = digest(row.digest)?;
        let receipt = JobReceiptV1::new(sequence, job.clone(), digest);
        if !matches!(
            row.state.as_str(),
            "queued" | "running" | "succeeded" | "failed" | "cancelled"
        ) {
            return Err(corrupt(StoreRecordKindV1::Job));
        }
        if matches!(row.state.as_str(), "queued" | "running") && row.terminal.is_some() {
            return Err(corrupt(StoreRecordKindV1::Job));
        }
        if terminal_state(&row.state) {
            let replay = validated_terminal(&receipt, &row.state, row.terminal.as_deref())?;
            verify_terminal_job_projection(
                &replay,
                &row.state,
                row.submitted_at,
                row.claimed_at,
                row.finished_at,
            )?;
            verify_terminal_idempotency(&transaction, &job, receipt.request_digest(), &replay)?;
            transaction.commit().map_err(storage)?;
            return Ok(CancelOutcomeV1::AlreadyTerminal(
                JobReceiptOrTerminalV1::TerminalReceipt(replay),
            ));
        }
        if row.state != "queued" {
            return Err(StoreErrorV1::AlreadyClaimedV1 { job_id: job });
        }
        if expected_job_revision != RevisionV1::new(sequence) {
            return Err(StoreErrorV1::CancellationLostV1 { job_id: job });
        }
        verify_live_idempotency(&transaction, &job, receipt.request_digest())?;
        let job_projection = PersistedTerminalJobProjectionV1::new(
            PersistedTerminalJobStateV1::Cancelled,
            epoch(row.submitted_at, StoreRecordKindV1::Job)?,
            None,
            now,
        )
        .map_err(|_| corrupt(StoreRecordKindV1::Job))?;
        let terminal = PersistedTerminalReceiptV1::new_with_projections(
            receipt.clone(),
            PersistedTerminalResultV1::Cancelled,
            job_projection,
            None,
        )
        .map_err(|_| corrupt(StoreRecordKindV1::Job))?;
        let encoded = encode_persisted_terminal_receipt_v1(&terminal)
            .map_err(|_| corrupt(StoreRecordKindV1::Job))?;
        let changed = transaction.execute(
            "UPDATE jobs SET state = 'cancelled', finished_at_ms = ?1, terminal_response_json = ?2 \
             WHERE job_id = ?3 AND state = 'queued'",
            params![sqlite_u64(now.get())?, encoded.as_str(), job.as_str()],
        ).map_err(storage)?;
        if changed != 1 {
            return Err(StoreErrorV1::CancellationLostV1 {
                job_id: job.clone(),
            });
        }
        let idempotency_changed = transaction
            .execute(
                "UPDATE idempotency_records SET terminal_response_json = ?1, updated_at_ms = ?2 \
                 WHERE job_id = ?3 AND terminal_response_json IS NULL",
                params![encoded.as_str(), sqlite_u64(now.get())?, job.as_str()],
            )
            .map_err(storage)?;
        if idempotency_changed != 1 {
            return Err(corrupt(StoreRecordKindV1::IdempotencyRecord));
        }
        transaction
            .execute(
                "UPDATE workspace_state SET updated_at_ms = ?1 WHERE singleton = 1",
                [sqlite_u64(now.get())?],
            )
            .map_err(storage)?;
        let prune_report =
            prune_terminal_history_transaction(&transaction, now, &self.options, true)?;
        record_prune_report(
            &transaction,
            &prune_report,
            Some(row.sequence),
            Some(job.as_str()),
        )?;
        transaction.commit().map_err(storage)?;
        Ok(CancelOutcomeV1::Cancelled(receipt))
    }

    fn commit_terminal(
        &self,
        claim: ClaimTokenV1,
        expected_workspace_revision: RevisionV1,
        transition: Option<StateTransitionV1>,
        result: TerminalResultV1,
        now: EpochMillisV1,
    ) -> Result<TerminalReceiptV1, StoreErrorV1> {
        self.require_identity(claim.identity())?;
        let mut connection = self.lock_connection()?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        self.trigger_failpoint(StoreFailpointV1::TerminalAfterTransactionBegin)?;
        let row: Option<RunningJobRow> = transaction
            .query_row(
                "SELECT workspace_sequence, request_digest, command_name, canonical_request_json, \
                 submitted_at_ms, claimed_at_ms, state FROM jobs WHERE job_id = ?1",
                [claim.job_id().as_str()],
                |row| {
                    Ok(RunningJobRow {
                        sequence: row.get(0)?,
                        digest: row.get(1)?,
                        command_name: row.get(2)?,
                        request: row.get(3)?,
                        submitted_at: row.get(4)?,
                        claimed_at: row.get(5)?,
                        state: row.get(6)?,
                    })
                },
            )
            .optional()
            .map_err(|error| storage_record(error, StoreRecordKindV1::Job))?;
        let Some(row) = row else {
            return Err(StoreErrorV1::JobNotFoundV1 {
                job_id: claim.job_id().clone(),
            });
        };
        let digest = digest(row.digest)?;
        let sequence =
            u64::try_from(row.sequence).map_err(|_| invariant(StoreInvariantV1::QueueSequence))?;
        if sequence == 0 {
            return Err(corrupt(StoreRecordKindV1::Job));
        }
        let submitted_at = epoch(row.submitted_at, StoreRecordKindV1::Job)?;
        let claimed_at = row
            .claimed_at
            .map(|value| epoch(value, StoreRecordKindV1::Job))
            .transpose()?;
        let valid_generation = claimed_at.map(|claimed_at| {
            self.claim_generation(
                claim.identity(),
                claim.job_id(),
                &digest,
                claimed_at,
                claim.worker(),
            )
        });
        if !matches!(
            row.state.as_str(),
            "queued" | "running" | "succeeded" | "failed" | "cancelled"
        ) {
            return Err(corrupt(StoreRecordKindV1::Job));
        }
        if row.state != "running" || valid_generation != Some(claim.job_revision()) {
            return Err(StoreErrorV1::ClaimStaleV1 {
                job_id: claim.job_id().clone(),
            });
        }
        let execution =
            decode_command_v1(&row.request).map_err(|_| corrupt(StoreRecordKindV1::Job))?;
        if command_name_v1(execution.command()) != row.command_name {
            return Err(corrupt(StoreRecordKindV1::Job));
        }
        verify_persisted_job_scope(&transaction, claim.job_id().as_str())?;
        validate_terminal_result_for_command_v1(execution.command(), &result)
            .map_err(|_| corrupt(StoreRecordKindV1::Job))?;
        let session_barrier = is_session_barrier(execution.command());
        let failed_session_barrier =
            session_barrier && matches!(&result, TerminalResultV1::Failure(_));
        if matches!(&result, TerminalResultV1::Success(_))
            && matches!(execution.command(), crate::CommandV1::WorkspaceResetAll)
        {
            return Err(invariant(StoreInvariantV1::TransitionMutationShape));
        }

        let (old_session, post_transition_session) = match &result {
            TerminalResultV1::Failure(_) => {
                if transition.is_some() {
                    return Err(invariant(StoreInvariantV1::TransitionMutationShape));
                }
                (None, None)
            }
            TerminalResultV1::Success(result) => {
                let current = load_current_session(&transaction)?;
                let actual_revision = current
                    .as_ref()
                    .map_or(RevisionV1::ZERO, podway_core::SessionAggregateV1::revision);
                if actual_revision != expected_workspace_revision {
                    return Err(StoreErrorV1::PreconditionConflictV1 {
                        expected: Some(expected_workspace_revision),
                        actual: Some(actual_revision),
                    });
                }
                validate_preconditions(execution.preconditions(), current.as_ref())?;
                let transition = transition
                    .as_ref()
                    .ok_or_else(|| invariant(StoreInvariantV1::TransitionMutationShape))?;
                if transition.previous_workspace_revision() != actual_revision {
                    return Err(StoreErrorV1::PreconditionConflictV1 {
                        expected: Some(transition.previous_workspace_revision()),
                        actual: Some(actual_revision),
                    });
                }
                validate_success_transition(
                    execution.command(),
                    result,
                    transition,
                    current.as_ref(),
                    &self.identity,
                )?;
                let old_session = if session_barrier {
                    capture_old_session_for_barrier(&transaction, row.sequence)?
                } else {
                    None
                };
                let post_transition_session = match transition.persisted_session_mutation() {
                    PersistedSessionMutationV1::Unchanged => current,
                    PersistedSessionMutationV1::Replace(aggregate)
                    | PersistedSessionMutationV1::ReplaceFresh(aggregate) => {
                        Some(aggregate.clone())
                    }
                    PersistedSessionMutationV1::Clear => None,
                };
                match transition.persisted_session_mutation() {
                    PersistedSessionMutationV1::Unchanged => {}
                    PersistedSessionMutationV1::Replace(aggregate)
                    | PersistedSessionMutationV1::ReplaceFresh(aggregate) => {
                        replace_current_session(&transaction, aggregate)?;
                    }
                    PersistedSessionMutationV1::Clear => {
                        transaction
                            .execute("DELETE FROM task_sessions WHERE singleton = 1", [])
                            .map_err(storage)?;
                    }
                }
                (old_session, post_transition_session)
            }
        };
        self.trigger_failpoint(
            StoreFailpointV1::TerminalAfterRelationalStateUpdatesBeforeJobTerminalUpdate,
        )?;
        let receipt = TerminalReceiptV1::new(
            JobReceiptV1::new(sequence, claim.job_id().clone(), digest),
            result,
        );
        let persisted_result = PersistedTerminalResultV1::from_terminal_result(receipt.result());
        let (state, job_state) = match &persisted_result {
            PersistedTerminalResultV1::Success(_) => {
                ("succeeded", PersistedTerminalJobStateV1::Succeeded)
            }
            PersistedTerminalResultV1::Failure(_) => {
                ("failed", PersistedTerminalJobStateV1::Failed)
            }
            PersistedTerminalResultV1::Cancelled => {
                return Err(invariant(StoreInvariantV1::TransitionMutationShape));
            }
        };
        let job_projection =
            PersistedTerminalJobProjectionV1::new(job_state, submitted_at, claimed_at, now)
                .map_err(|_| corrupt(StoreRecordKindV1::Job))?;
        let session_projection =
            terminal_session_projection(&persisted_result, post_transition_session.as_ref())?;
        let persisted_receipt = PersistedTerminalReceiptV1::new_with_projections(
            receipt.job().clone(),
            persisted_result,
            job_projection,
            session_projection,
        )
        .map_err(|_| corrupt(StoreRecordKindV1::Job))?;
        let encoded = encode_persisted_terminal_receipt_v1(&persisted_receipt)
            .map_err(|_| corrupt(StoreRecordKindV1::Job))?;
        let changed = transaction
            .execute(
                "UPDATE jobs SET state = ?1, finished_at_ms = ?2, terminal_response_json = ?3 \
                 WHERE job_id = ?4 AND state = 'running'",
                params![
                    state,
                    sqlite_u64(now.get())?,
                    &encoded,
                    claim.job_id().as_str(),
                ],
            )
            .map_err(storage)?;
        if changed != 1 {
            return Err(StoreErrorV1::ClaimStaleV1 {
                job_id: claim.job_id().clone(),
            });
        }
        self.trigger_failpoint(StoreFailpointV1::TerminalAfterJobTerminalUpdateBeforeCommit)?;
        let idempotency_changed = transaction
            .execute(
                "UPDATE idempotency_records SET terminal_response_json = ?1, updated_at_ms = ?2 \
                 WHERE job_id = ?3 AND terminal_response_json IS NULL",
                params![&encoded, sqlite_u64(now.get())?, claim.job_id().as_str()],
            )
            .map_err(storage)?;
        if idempotency_changed != 1 {
            return Err(corrupt(StoreRecordKindV1::IdempotencyRecord));
        }
        transaction
            .execute(
                "UPDATE workspace_state SET updated_at_ms = ?1 WHERE singleton = 1",
                [sqlite_u64(now.get())?],
            )
            .map_err(storage)?;
        if let Some((old_session_id, old_snapshot_id)) = old_session {
            let cleanup_report =
                cleanup_old_session_barrier(&transaction, &old_session_id, &old_snapshot_id)?;
            record_session_barrier_cleanup(
                &transaction,
                row.sequence,
                claim.job_id().as_str(),
                now,
                &cleanup_report,
            )?;
        }
        if !failed_session_barrier {
            let prune_report =
                prune_terminal_history_transaction(&transaction, now, &self.options, true)?;
            record_prune_report(
                &transaction,
                &prune_report,
                Some(row.sequence),
                Some(claim.job_id().as_str()),
            )?;
        }
        if matches!(receipt.result(), TerminalResultV1::Failure(_)) {
            self.trigger_failpoint(StoreFailpointV1::TerminalFailureBeforeCommit)?;
        }
        self.trigger_failpoint(StoreFailpointV1::TerminalBeforeCommit)?;
        transaction.commit().map_err(storage)?;
        self.trigger_failpoint(StoreFailpointV1::TerminalAfterCommitBeforeResponse)?;
        Ok(receipt)
    }

    fn read_workspace_view(
        &self,
        identity: &DurableWorktreeIdentityV1,
    ) -> Result<WorkspaceViewV1, StoreErrorV1> {
        self.require_identity(identity)?;
        let mut connection = self.lock_connection()?;
        let transaction = connection.transaction().map_err(storage)?;
        let current_session = load_current_session(&transaction)?;
        let state = load_workspace_state(&transaction, self.identity.workspace_uuid().clone())?;
        let queued: i64 = transaction
            .query_row(
                "SELECT COUNT(*) FROM jobs WHERE state = 'queued'",
                [],
                |row| row.get(0),
            )
            .map_err(storage)?;
        let running: Option<String> = transaction
            .query_row(
                "SELECT job_id FROM jobs WHERE state = 'running' ORDER BY workspace_sequence LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage)?;
        let (latest_workspace_sequence, observed): (i64, i64) = transaction
            .query_row(
                "SELECT next_workspace_sequence, updated_at_ms FROM workspace_state WHERE singleton = 1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(|error| storage_record(error, StoreRecordKindV1::Workspace))?;
        let view = WorkspaceViewV1::new_coherent(
            self.identity.clone(),
            state,
            current_session,
            u32::try_from(queued).map_err(|_| invariant(StoreInvariantV1::QueueSequence))?,
            running.map(job_id).transpose()?,
            u64::try_from(latest_workspace_sequence)
                .map_err(|_| corrupt(StoreRecordKindV1::Workspace))?,
            epoch(observed, StoreRecordKindV1::Workspace)?,
        );
        transaction.commit().map_err(storage)?;
        Ok(view)
    }
}

impl StoreIdempotencyReadContractV1 for SqliteStoreV1 {
    fn read_idempotent_outcome(
        &self,
        identity: &DurableWorktreeIdentityV1,
        idempotency_key: &crate::IdempotencyKeyV1,
        request_digest: &CanonicalRequestDigestV1,
    ) -> Result<Option<AdmitOutcomeV1>, StoreErrorV1> {
        self.require_identity(identity)?;
        let mut connection = self.lock_connection()?;
        let transaction = connection.transaction().map_err(storage)?;
        let existing: Option<(String, String, Option<String>)> = transaction
            .query_row(
                "SELECT request_digest, job_id, terminal_response_json FROM idempotency_records \
                 WHERE idempotency_key = ?1",
                [idempotency_key.as_str()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(|error| storage_record(error, StoreRecordKindV1::IdempotencyRecord))?;
        let Some((stored_digest, job_id, terminal)) = existing else {
            transaction.commit().map_err(storage)?;
            return Ok(None);
        };
        let stored_digest = digest(stored_digest)?;
        if stored_digest != *request_digest {
            return Err(StoreErrorV1::IdempotencyDigestConflictV1 {
                expected: stored_digest,
                actual: request_digest.clone(),
            });
        }
        let outcome = replay_for_idempotency(&transaction, &job_id, &stored_digest, terminal)?;
        transaction.commit().map_err(storage)?;
        Ok(Some(AdmitOutcomeV1::Existing(outcome)))
    }
}
impl StoreReadContractV1 for SqliteStoreV1 {
    fn read_session_aggregate(
        &self,
        identity: &DurableWorktreeIdentityV1,
    ) -> Result<Option<podway_core::SessionAggregateV1>, StoreErrorV1> {
        SqliteStoreV1::read_session_aggregate(self, identity)
    }

    fn read_job(
        &self,
        identity: &DurableWorktreeIdentityV1,
        job: &JobIdV1,
    ) -> Result<Option<JobViewV1>, StoreErrorV1> {
        self.require_identity(identity)?;
        let mut connection = self.lock_connection()?;
        let transaction = connection.transaction().map_err(storage)?;
        let row = transaction
            .query_row(
                "SELECT job_id, workspace_sequence, request_digest, command_name, canonical_request_json, \
                 state, session_id, submitted_at_ms, claimed_at_ms, finished_at_ms, terminal_response_json \
                 FROM jobs WHERE job_id = ?1",
                [job.as_str()],
                JobViewRowV1::from_row,
            )
            .optional()
            .map_err(|error| storage_record(error, StoreRecordKindV1::Job))?;
        let view = row
            .map(|row| decode_job_view_v1(&transaction, row))
            .transpose()?;
        transaction.commit().map_err(storage)?;
        Ok(view)
    }

    fn list_jobs(
        &self,
        identity: &DurableWorktreeIdentityV1,
        query: JobListQueryV1,
    ) -> Result<Vec<JobViewV1>, StoreErrorV1> {
        self.require_identity(identity)?;
        let mut connection = self.lock_connection()?;
        let transaction = connection.transaction().map_err(storage)?;
        let views = {
            let mut statement = transaction
                .prepare(
                    "SELECT job_id, workspace_sequence, request_digest, command_name, canonical_request_json, \
                     state, session_id, submitted_at_ms, claimed_at_ms, finished_at_ms, terminal_response_json \
                     FROM jobs ORDER BY workspace_sequence ASC LIMIT ?1",
                )
                .map_err(|error| storage_record(error, StoreRecordKindV1::Job))?;
            let mut rows = statement
                .query([i64::from(query.limit())])
                .map_err(|error| storage_record(error, StoreRecordKindV1::Job))?;
            let mut views = Vec::with_capacity(query.limit() as usize);
            while let Some(row) = rows
                .next()
                .map_err(|error| storage_record(error, StoreRecordKindV1::Job))?
            {
                let row = JobViewRowV1::from_row(row)
                    .map_err(|error| storage_record(error, StoreRecordKindV1::Job))?;
                views.push(decode_job_view_v1(&transaction, row)?);
            }
            views
        };
        transaction.commit().map_err(storage)?;
        Ok(views)
    }
}

const TERMINAL_JOB_PROTECTED_COUNT_V1: i64 = 100;
const TERMINAL_JOB_ABSOLUTE_COUNT_V1: i64 = 1_000;
const JOURNAL_PROTECTED_COUNT_V1: i64 = 200;
const JOURNAL_ABSOLUTE_COUNT_V1: i64 = 10_000;
const TERMINAL_HISTORY_MAX_AGE_MS_V1: u64 = 7 * 24 * 60 * 60 * 1_000;
const ORPHAN_RECEIPT_MAX_AGE_MS_V1: u64 = 30 * 24 * 60 * 60 * 1_000;
const ORPHAN_RECEIPT_PROTECTED_COUNT_V1: i64 = 100;

fn recover_running_connection(
    connection: &mut Connection,
    options: &SqliteStoreOptionsV1,
    now: EpochMillisV1,
) -> Result<RecoveryReportV1, StoreErrorV1> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage)?;
    let running: Vec<(String, i64)> = {
        let mut statement = transaction
            .prepare(
                "SELECT job_id, workspace_sequence FROM jobs \
                 WHERE state = 'running' AND finished_at_ms IS NULL \
                 ORDER BY workspace_sequence",
            )
            .map_err(storage)?;
        let rows = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(storage)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(storage)?
    };
    let mut journaled = 0u32;
    let mut requeued = 0u32;
    let recorded_at = sqlite_u64(now.get())?;
    for (job_id, workspace_sequence) in &running {
        let inserted = transaction
            .execute(
                "INSERT INTO operational_journal \
                 (recorded_at_ms, level, event_name, workspace_sequence, job_id, summary, details_json) \
                 VALUES (?1, 'warn', 'job.recovered', ?2, ?3, \
                 'running job requeued during startup recovery', NULL)",
                params![recorded_at, workspace_sequence, job_id.as_str()],
            )
            .map_err(storage)?;
        journaled = journaled
            .checked_add(
                u32::try_from(inserted).map_err(|_| invariant(StoreInvariantV1::RecoveryParity))?,
            )
            .ok_or_else(|| invariant(StoreInvariantV1::RecoveryParity))?;
        let changed = transaction
            .execute(
                "UPDATE jobs SET state = 'queued', claimed_at_ms = NULL \
                 WHERE job_id = ?1 AND state = 'running' AND finished_at_ms IS NULL",
                [job_id.as_str()],
            )
            .map_err(storage)?;
        requeued = requeued
            .checked_add(
                u32::try_from(changed).map_err(|_| invariant(StoreInvariantV1::RecoveryParity))?,
            )
            .ok_or_else(|| invariant(StoreInvariantV1::RecoveryParity))?;
    }
    let expected =
        u32::try_from(running.len()).map_err(|_| invariant(StoreInvariantV1::RecoveryParity))?;
    if journaled != expected || requeued != expected {
        return Err(invariant(StoreInvariantV1::RecoveryParity));
    }
    let remaining_running: i64 = transaction
        .query_row(
            "SELECT COUNT(*) FROM jobs WHERE state = 'running'",
            [],
            |row| row.get(0),
        )
        .map_err(storage)?;
    if remaining_running != 0 {
        return Err(invariant(StoreInvariantV1::RecoveryParity));
    }
    transaction
        .execute(
            "UPDATE workspace_state SET updated_at_ms = ?1 WHERE singleton = 1",
            [recorded_at],
        )
        .map_err(storage)?;
    let prune_report = prune_terminal_history_transaction(&transaction, now, options, true)?;
    record_prune_report(&transaction, &prune_report, None, None)?;
    options.trigger_failpoint(StoreFailpointV1::RecoveryBeforeCommit)?;
    transaction.commit().map_err(storage)?;
    options.trigger_failpoint(StoreFailpointV1::RecoveryAfterCommitBeforeReturn)?;
    Ok(RecoveryReportV1::new(requeued, now))
}

fn prune_terminal_history_transaction(
    transaction: &Transaction<'_>,
    now: EpochMillisV1,
    options: &SqliteStoreOptionsV1,
    journal_report_follows: bool,
) -> Result<PruneReportV1, StoreErrorV1> {
    let deleted_terminal_jobs = prune_terminal_jobs(transaction, now)?;
    let deleted_journal_entries =
        prune_operational_journal(transaction, now, journal_report_follows)?;
    let deleted_orphan_receipts = prune_orphan_workspace_receipts(transaction, now)?;
    if deleted_terminal_jobs != 0 || deleted_journal_entries != 0 || deleted_orphan_receipts != 0 {
        options.trigger_failpoint(StoreFailpointV1::PruneAfterDeleteStagingBeforeCommit)?;
    }
    Ok(PruneReportV1::new(
        deleted_terminal_jobs,
        deleted_journal_entries,
        deleted_orphan_receipts,
        now,
    ))
}

fn record_prune_report(
    transaction: &Transaction<'_>,
    report: &PruneReportV1,
    workspace_sequence: Option<i64>,
    job_id: Option<&str>,
) -> Result<(), StoreErrorV1> {
    if report.deleted_terminal_jobs() == 0
        && report.deleted_journal_entries() == 0
        && report.deleted_orphan_workspace_receipts() == 0
    {
        return Ok(());
    }
    let summary = format!(
        "terminal_jobs_deleted={}; journal_entries_deleted={}; orphan_workspace_receipts_deleted={}",
        report.deleted_terminal_jobs(),
        report.deleted_journal_entries(),
        report.deleted_orphan_workspace_receipts(),
    );
    let inserted = transaction
        .execute(
            "INSERT INTO operational_journal \
             (recorded_at_ms, level, event_name, workspace_sequence, job_id, summary, details_json) \
             VALUES (?1, 'info', 'retention.pruned', ?2, ?3, ?4, NULL)",
            params![
                sqlite_u64(report.pruned_at().get())?,
                workspace_sequence,
                job_id,
                summary,
            ],
        )
        .map_err(storage)?;
    if inserted != 1 {
        return Err(invariant(StoreInvariantV1::RetentionAccounting));
    }
    Ok(())
}
type TerminalPruneCandidateV1 = (
    String,
    i64,
    String,
    String,
    Option<String>,
    i64,
    Option<i64>,
    Option<i64>,
);

fn prune_terminal_jobs(
    transaction: &Transaction<'_>,
    now: EpochMillisV1,
) -> Result<u32, StoreErrorV1> {
    let cutoff = sqlite_u64(now.get().saturating_sub(TERMINAL_HISTORY_MAX_AGE_MS_V1))?;
    let candidates: Vec<TerminalPruneCandidateV1> = {
        let mut statement = transaction
            .prepare(
                "WITH ranked AS (
                    SELECT job_id, workspace_sequence, request_digest, state, terminal_response_json,
                           submitted_at_ms, claimed_at_ms, finished_at_ms,
                           ROW_NUMBER() OVER (
                               ORDER BY finished_at_ms DESC, workspace_sequence DESC
                           ) AS terminal_rank
                    FROM jobs
                    WHERE state IN ('succeeded', 'failed', 'cancelled')
                )
                SELECT job_id, workspace_sequence, request_digest, state, terminal_response_json,
                       submitted_at_ms, claimed_at_ms, finished_at_ms
                FROM ranked
                WHERE terminal_rank > ?1
                  AND (finished_at_ms < ?2 OR terminal_rank > ?3)
                ORDER BY finished_at_ms ASC, workspace_sequence ASC",
            )
            .map_err(storage)?;
        let rows = statement
            .query_map(
                params![
                    TERMINAL_JOB_PROTECTED_COUNT_V1,
                    cutoff,
                    TERMINAL_JOB_ABSOLUTE_COUNT_V1,
                ],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                        row.get(7)?,
                    ))
                },
            )
            .map_err(storage)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(storage)?
    };
    let mut deleted = 0u32;
    for (job, sequence, stored_digest, state, encoded, submitted_at, claimed_at, finished_at) in
        candidates
    {
        let job_id = job_id(job)?;
        let request_digest = digest(stored_digest)?;
        let receipt = JobReceiptV1::new(
            u64::try_from(sequence)
                .map_err(|_| invariant(StoreInvariantV1::RetentionAccounting))?,
            job_id.clone(),
            request_digest.clone(),
        );
        let terminal = validated_terminal(&receipt, &state, encoded.as_deref())?;
        verify_terminal_job_projection(&terminal, &state, submitted_at, claimed_at, finished_at)?;
        verify_terminal_idempotency(transaction, &job_id, &request_digest, &terminal)?;
        let changed = transaction
            .execute(
                "DELETE FROM jobs \
                 WHERE job_id = ?1 AND state IN ('succeeded', 'failed', 'cancelled')",
                [job_id.as_str()],
            )
            .map_err(storage)?;
        if changed != 1 {
            return Err(invariant(StoreInvariantV1::RetentionAccounting));
        }
        deleted = deleted
            .checked_add(1)
            .ok_or_else(|| invariant(StoreInvariantV1::RetentionAccounting))?;
    }
    Ok(deleted)
}

fn prune_operational_journal(
    transaction: &Transaction<'_>,
    now: EpochMillisV1,
    reserve_report_slot: bool,
) -> Result<u32, StoreErrorV1> {
    let cutoff = sqlite_u64(now.get().saturating_sub(TERMINAL_HISTORY_MAX_AGE_MS_V1))?;
    let candidates: Vec<i64> = {
        let mut statement = transaction
            .prepare(
                "WITH ranked AS (
                    SELECT journal_id, recorded_at_ms,
                           ROW_NUMBER() OVER (
                               ORDER BY recorded_at_ms DESC, journal_id DESC
                           ) AS journal_rank
                    FROM operational_journal
                )
                SELECT journal_id
                FROM ranked
                WHERE journal_rank > ?1
                  AND (recorded_at_ms < ?2 OR journal_rank > ?3)
                ORDER BY recorded_at_ms ASC, journal_id ASC",
            )
            .map_err(storage)?;
        let rows = statement
            .query_map(
                params![
                    JOURNAL_PROTECTED_COUNT_V1,
                    cutoff,
                    if reserve_report_slot {
                        JOURNAL_ABSOLUTE_COUNT_V1 - 1
                    } else {
                        JOURNAL_ABSOLUTE_COUNT_V1
                    },
                ],
                |row| row.get(0),
            )
            .map_err(storage)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(storage)?
    };
    let mut deleted = 0u32;
    for journal_id in candidates {
        let changed = transaction
            .execute(
                "DELETE FROM operational_journal WHERE journal_id = ?1",
                [journal_id],
            )
            .map_err(storage)?;
        if changed != 1 {
            return Err(invariant(StoreInvariantV1::RetentionAccounting));
        }
        deleted = deleted
            .checked_add(1)
            .ok_or_else(|| invariant(StoreInvariantV1::RetentionAccounting))?;
    }
    Ok(deleted)
}

fn prune_orphan_workspace_receipts(
    transaction: &Transaction<'_>,
    now: EpochMillisV1,
) -> Result<u32, StoreErrorV1> {
    let cutoff = sqlite_u64(now.get().saturating_sub(ORPHAN_RECEIPT_MAX_AGE_MS_V1))?;
    let max_workspace_sequence = workspace_sequence_upper_bound(transaction)?;
    let candidates: Vec<(String, String, String, String)> = {
        let mut statement = transaction
            .prepare(
                "WITH ranked AS (
                    SELECT idempotency_key, job_id, request_digest, terminal_response_json, updated_at_ms,
                           ROW_NUMBER() OVER (
                               ORDER BY updated_at_ms DESC, idempotency_key DESC
                           ) AS receipt_rank
                    FROM idempotency_records AS records
                    WHERE scope_kind = 'workspace'
                      AND terminal_response_json IS NOT NULL
                      AND NOT EXISTS (
                          SELECT 1 FROM jobs WHERE jobs.job_id = records.job_id
                      )
                )
                SELECT idempotency_key, job_id, request_digest, terminal_response_json
                FROM ranked
                WHERE receipt_rank > ?1 OR updated_at_ms < ?2
                ORDER BY updated_at_ms ASC, idempotency_key ASC",
            )
            .map_err(storage)?;
        let rows = statement
            .query_map(params![ORPHAN_RECEIPT_PROTECTED_COUNT_V1, cutoff], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .map_err(storage)?;
        rows.collect::<Result<Vec<_>, _>>().map_err(storage)?
    };
    let mut deleted = 0u32;
    for (key, stored_job_id, stored_digest, encoded) in candidates {
        let request_digest = digest(stored_digest)?;
        let receipt = decode_terminal_receipt_v1(&encoded)
            .map_err(|_| corrupt(StoreRecordKindV1::IdempotencyRecord))?;
        if receipt.job().identity_sequence() == 0
            || receipt.job().identity_sequence() > max_workspace_sequence
            || receipt.job().job_id().as_str() != stored_job_id
            || receipt.job().request_digest() != &request_digest
        {
            return Err(corrupt(StoreRecordKindV1::IdempotencyRecord));
        }
        let changed = transaction
            .execute(
                "DELETE FROM idempotency_records \
                 WHERE idempotency_key = ?1 AND scope_kind = 'workspace' \
                 AND terminal_response_json = ?2",
                params![key, encoded],
            )
            .map_err(storage)?;
        if changed != 1 {
            return Err(invariant(StoreInvariantV1::RetentionAccounting));
        }
        deleted = deleted
            .checked_add(1)
            .ok_or_else(|| invariant(StoreInvariantV1::RetentionAccounting))?;
    }
    Ok(deleted)
}

fn capture_old_session_for_barrier(
    transaction: &Transaction<'_>,
    barrier_sequence: i64,
) -> Result<Option<(String, String)>, StoreErrorV1> {
    let earlier_nonterminal: i64 = transaction
        .query_row(
            "SELECT EXISTS(
                SELECT 1 FROM jobs
                WHERE workspace_sequence < ?1 AND state IN ('queued', 'running')
            )",
            [barrier_sequence],
            |row| row.get(0),
        )
        .map_err(storage)?;
    if earlier_nonterminal != 0 {
        return Err(invariant(StoreInvariantV1::TransitionMutationShape));
    }
    let old_session: Option<(String, String)> = transaction
        .query_row(
            "SELECT session_id, procedure_snapshot_id FROM task_sessions WHERE singleton = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(storage)?;
    if let Some((session_id, _)) = &old_session {
        let nonterminal_old_session_job: i64 = transaction
            .query_row(
                "SELECT EXISTS(
                    SELECT 1 FROM jobs
                    WHERE session_id = ?1 AND state IN ('queued', 'running')
                )",
                [session_id.as_str()],
                |row| row.get(0),
            )
            .map_err(storage)?;
        if nonterminal_old_session_job != 0 {
            return Err(invariant(StoreInvariantV1::TransitionMutationShape));
        }
    }
    Ok(old_session)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SessionBarrierCleanupReportV1 {
    deleted_journal_entries: u32,
    deleted_terminal_jobs: u32,
    deleted_idempotency_records: u32,
    deleted_snapshots: u32,
}

fn verify_session_cleanup_job_scopes(
    transaction: &Transaction<'_>,
    session_id: &str,
) -> Result<(), StoreErrorV1> {
    let mut statement = transaction
        .prepare("SELECT job_id FROM jobs WHERE session_id = ?1")
        .map_err(storage)?;
    let job_ids = statement
        .query_map([session_id], |row| row.get::<_, String>(0))
        .map_err(storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage)?;
    drop(statement);
    for job_id in job_ids {
        verify_persisted_job_scope(transaction, &job_id)?;
    }
    Ok(())
}
fn cleanup_old_session_barrier(
    transaction: &Transaction<'_>,
    old_session_id: &str,
    old_snapshot_id: &str,
) -> Result<SessionBarrierCleanupReportV1, StoreErrorV1> {
    verify_session_cleanup_job_scopes(transaction, old_session_id)?;
    let deleted_journal_entries = transaction
        .execute(
            "DELETE FROM operational_journal
             WHERE job_id IN (SELECT job_id FROM jobs WHERE session_id = ?1)",
            [old_session_id],
        )
        .map_err(storage)?;
    let deleted_terminal_jobs = transaction
        .execute(
            "DELETE FROM jobs
             WHERE session_id = ?1 AND state IN ('succeeded', 'failed', 'cancelled')",
            [old_session_id],
        )
        .map_err(storage)?;
    let deleted_idempotency_records = transaction
        .execute(
            "DELETE FROM idempotency_records
             WHERE scope_kind = 'session' AND scope_session_id = ?1",
            [old_session_id],
        )
        .map_err(storage)?;
    let deleted_snapshots = transaction
        .execute(
            "DELETE FROM procedure_snapshots
             WHERE snapshot_id = ?1
               AND NOT EXISTS (
                   SELECT 1 FROM task_sessions WHERE procedure_snapshot_id = ?1
               )",
            [old_snapshot_id],
        )
        .map_err(storage)?;
    Ok(SessionBarrierCleanupReportV1 {
        deleted_journal_entries: retention_count(deleted_journal_entries)?,
        deleted_terminal_jobs: retention_count(deleted_terminal_jobs)?,
        deleted_idempotency_records: retention_count(deleted_idempotency_records)?,
        deleted_snapshots: retention_count(deleted_snapshots)?,
    })
}

fn record_session_barrier_cleanup(
    transaction: &Transaction<'_>,
    workspace_sequence: i64,
    job_id: &str,
    now: EpochMillisV1,
    report: &SessionBarrierCleanupReportV1,
) -> Result<(), StoreErrorV1> {
    let summary = format!(
        "journal_entries_deleted={}; terminal_jobs_deleted={}; idempotency_records_deleted={}; snapshots_deleted={}",
        report.deleted_journal_entries,
        report.deleted_terminal_jobs,
        report.deleted_idempotency_records,
        report.deleted_snapshots,
    );
    let inserted = transaction
        .execute(
            "INSERT INTO operational_journal \
             (recorded_at_ms, level, event_name, workspace_sequence, job_id, summary, details_json) \
             VALUES (?1, 'info', 'session.barrier.cleanup', ?2, ?3, ?4, NULL)",
            params![sqlite_u64(now.get())?, workspace_sequence, job_id, summary],
        )
        .map_err(storage)?;
    if inserted != 1 {
        return Err(invariant(StoreInvariantV1::RetentionAccounting));
    }
    Ok(())
}

fn retention_count(count: usize) -> Result<u32, StoreErrorV1> {
    u32::try_from(count).map_err(|_| invariant(StoreInvariantV1::RetentionAccounting))
}

fn initialize_new_database_atomically(
    path: &Path,
    root: &ValidatedWorkspaceRootV1,
    identity: &DurableWorktreeIdentityV1,
    options: &SqliteStoreOptionsV1,
    now: EpochMillisV1,
) -> Result<Connection, StoreErrorV1> {
    let temporary = create_temporary_database(path, now)?;
    let mut cleanup_armed = false;
    let result = (|| {
        let connection = open_or_initialize_with_temporary_cleanup_arm_v1(
            &temporary.database,
            root,
            identity,
            options,
            now,
            Some(&mut cleanup_armed),
        )?;
        checkpoint_close_and_sync(connection, &temporary.database)?;
        options.trigger_failpoint(StoreFailpointV1::SchemaAfterInitializationBeforePublication)?;
        match publish_temporary_database_no_clobber(
            &temporary.database,
            &temporary._database_file,
            path,
            options,
            false,
        ) {
            Ok(outcome) => Ok(outcome),
            Err(StoreErrorV1::StorageUnavailableV1 { .. })
                if inspect_publication_destination(path)? == DatabasePathStateV1::Existing =>
            {
                Ok(PublicationOutcomeV1::Existing)
            }
            Err(error) => Err(error),
        }
    })();
    let cleanup = cleanup_temporary_database(&temporary, cleanup_armed);
    combine_operation_and_cleanup(result, cleanup)?;
    open_or_initialize_v1(path, root, identity, options, now)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PublicationOutcomeV1 {
    Published,
    Existing,
}

struct TemporaryDatabaseV1 {
    database: PathBuf,
    wal: PathBuf,
    shm: PathBuf,
    marker: PathBuf,
    _database_file: File,
    _wal_file: File,
    _shm_file: File,
    _marker_lock: File,
}

fn combine_operation_and_cleanup<T>(
    operation: Result<T, StoreErrorV1>,
    cleanup: Result<(), StoreErrorV1>,
) -> Result<T, StoreErrorV1> {
    match (operation, cleanup) {
        (Ok(value), Ok(())) => Ok(value),
        (Ok(_), Err(cleanup)) => Err(cleanup),
        (Err(primary), Ok(())) => Err(primary),
        (Err(primary), Err(cleanup)) => Err(StoreErrorV1::PrimaryOperationAndCleanupFailureV1 {
            primary: Box::new(primary),
            cleanup: Box::new(cleanup),
        }),
    }
}
fn operation_error_with_cleanup(
    primary: StoreErrorV1,
    cleanup: Result<(), StoreErrorV1>,
) -> StoreErrorV1 {
    match combine_operation_and_cleanup::<()>(Err(primary), cleanup) {
        Err(error) => error,
        Ok(()) => unreachable!("an operation error cannot become success during cleanup"),
    }
}

fn cleanup_temporary_creation_files(
    marker: &Path,
    marker_lock: &File,
    database: &Path,
    database_file: &File,
    wal: &Path,
    wal_file: &File,
) -> Result<(), StoreErrorV1> {
    combine_operation_and_cleanup(
        remove_owned_temporary_file(wal, wal_file, false),
        combine_operation_and_cleanup(
            remove_owned_temporary_file(database, database_file, false),
            remove_owned_temporary_file(marker, marker_lock, false),
        ),
    )
}

struct ResetSeedContextV1<'a> {
    root: &'a ValidatedWorkspaceRootV1,
    identity: &'a DurableWorktreeIdentityV1,
    options: &'a SqliteStoreOptionsV1,
    request: &'a AdmitRequestV1,
    receipt: &'a TerminalReceiptV1,
    canonical_request: &'a str,
    terminal_response: &'a str,
    now: EpochMillisV1,
}

type ResetJobRowV1 = (
    String,
    i64,
    String,
    String,
    String,
    String,
    String,
    Option<String>,
    i64,
    Option<i64>,
    Option<i64>,
    Option<String>,
);

type ResetJournalRowV1 = (
    i64,
    String,
    String,
    Option<i64>,
    Option<String>,
    String,
    Option<String>,
    i64,
);

fn create_temporary_database(
    path: &Path,
    now: EpochMillisV1,
) -> Result<TemporaryDatabaseV1, StoreErrorV1> {
    validate_database_parent_path_v1(path)?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .ok_or_else(|| invariant(StoreInvariantV1::Publication))?;
    for attempt in 0..128u32 {
        let temporary = parent.join(temporary_database_name(file_name, now, attempt));
        let marker = temporary_ownership_marker_path(&temporary);
        let Some(mut marker_lock) = create_new_ownership_marker(&marker)? else {
            continue;
        };
        let database_file = match create_new_private_temporary_file(&temporary, false) {
            Ok(Some(file)) => file,
            Ok(None) => {
                remove_owned_temporary_file(&marker, &marker_lock, false)?;
                continue;
            }
            Err(error) => {
                return Err(operation_error_with_cleanup(
                    error,
                    remove_owned_temporary_file(&marker, &marker_lock, false),
                ));
            }
        };
        if let Err(error) = write_temporary_ownership_marker_v1(&mut marker_lock, &database_file) {
            return Err(operation_error_with_cleanup(
                error,
                combine_operation_and_cleanup(
                    remove_owned_temporary_file(&temporary, &database_file, false),
                    remove_owned_temporary_file(&marker, &marker_lock, false),
                ),
            ));
        }
        let wal = sqlite_sidecar_path(&temporary, "-wal");
        let wal_file = match create_new_private_temporary_file(&wal, false) {
            Ok(Some(file)) => file,
            Ok(None) => {
                combine_operation_and_cleanup(
                    remove_owned_temporary_file(&temporary, &database_file, false),
                    remove_owned_temporary_file(&marker, &marker_lock, false),
                )?;
                continue;
            }
            Err(error) => {
                return Err(operation_error_with_cleanup(
                    error,
                    combine_operation_and_cleanup(
                        remove_owned_temporary_file(&temporary, &database_file, false),
                        remove_owned_temporary_file(&marker, &marker_lock, false),
                    ),
                ));
            }
        };
        let shm = sqlite_sidecar_path(&temporary, "-shm");
        let shm_file = match create_new_private_temporary_file(&shm, false) {
            Ok(Some(file)) => file,
            Ok(None) => {
                cleanup_temporary_creation_files(
                    &marker,
                    &marker_lock,
                    &temporary,
                    &database_file,
                    &wal,
                    &wal_file,
                )?;
                continue;
            }
            Err(error) => {
                return Err(operation_error_with_cleanup(
                    error,
                    cleanup_temporary_creation_files(
                        &marker,
                        &marker_lock,
                        &temporary,
                        &database_file,
                        &wal,
                        &wal_file,
                    ),
                ));
            }
        };
        return Ok(TemporaryDatabaseV1 {
            database: temporary,
            wal,
            shm,
            marker,
            _database_file: database_file,
            _wal_file: wal_file,
            _shm_file: shm_file,
            _marker_lock: marker_lock,
        });
    }
    Err(StoreErrorV1::StorageUnavailableV1 {
        reason: StoreUnavailableReasonV1::StorageIo,
    })
}
fn create_new_ownership_marker(path: &Path) -> Result<Option<File>, StoreErrorV1> {
    create_new_private_temporary_file(path, true)
}

fn temporary_database_name(
    file_name: &std::ffi::OsStr,
    now: EpochMillisV1,
    attempt: u32,
) -> std::ffi::OsString {
    let mut temporary_name = std::ffi::OsString::from(".");
    temporary_name.push(file_name);
    temporary_name.push(".");
    temporary_name.push(std::process::id().to_string());
    temporary_name.push(".");
    temporary_name.push(now.get().to_string());
    temporary_name.push(".");
    temporary_name.push(attempt.to_string());
    temporary_name.push(".tmp");
    temporary_name
}

fn create_new_private_temporary_file(
    path: &Path,
    lock: bool,
) -> Result<Option<File>, StoreErrorV1> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    #[cfg(target_os = "macos")]
    if lock {
        options.custom_flags(0x20);
    }
    match options.open(path) {
        Ok(file) => {
            validate_existing_regular_private_file_v1(path)?;
            #[cfg(all(unix, not(target_os = "macos")))]
            if lock {
                file.lock().map_err(storage_io)?;
            }
            Ok(Some(file))
        }
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {
            validate_existing_regular_private_file_v1(path)?;
            Ok(None)
        }
        Err(error) => Err(storage_io(error)),
    }
}

fn checkpoint_wal_truncate(connection: &Connection) -> Result<(), StoreErrorV1> {
    let (busy, frames_in_wal, checkpointed_frames): (i64, i64, i64) = connection
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .map_err(storage)?;
    if busy != 0 || frames_in_wal != 0 || checkpointed_frames != 0 {
        return Err(StoreErrorV1::StorageUnavailableV1 {
            reason: StoreUnavailableReasonV1::Busy,
        });
    }
    Ok(())
}

fn verify_checkpointed_wal_is_empty(path: &Path) -> Result<(), StoreErrorV1> {
    let wal = sqlite_sidecar_path(path, "-wal");
    match fs::symlink_metadata(&wal) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() != 0 {
                return Err(StoreErrorV1::StorageUnavailableV1 {
                    reason: StoreUnavailableReasonV1::Busy,
                });
            }
            validate_existing_regular_private_file_v1(&wal)
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(storage_io(error)),
    }
}

fn checkpoint_close_and_sync(connection: Connection, path: &Path) -> Result<(), StoreErrorV1> {
    checkpoint_wal_truncate(&connection)?;
    connection.close().map_err(|(_, error)| storage(error))?;
    verify_checkpointed_wal_is_empty(path)?;
    validate_existing_database_path_v1(path)?;
    File::open(path)
        .map_err(storage_io)?
        .sync_all()
        .map_err(storage_io)
}

fn inspect_publication_destination(
    destination: &Path,
) -> Result<DatabasePathStateV1, StoreErrorV1> {
    const MAX_ACTIVE_PUBLICATION_RETRIES_V1: u16 = 1_000;
    for _ in 0..MAX_ACTIVE_PUBLICATION_RETRIES_V1 {
        match recover_interrupted_publication_v1(destination) {
            Ok(()) => return inspect_database_path_v1(destination),
            Err(StoreErrorV1::StorageUnavailableV1 {
                reason: StoreUnavailableReasonV1::Busy,
            }) => std::thread::sleep(std::time::Duration::from_millis(1)),
            Err(error) => return Err(error),
        }
    }
    Err(StoreErrorV1::StorageUnavailableV1 {
        reason: StoreUnavailableReasonV1::Busy,
    })
}

fn publish_temporary_database_no_clobber(
    temporary: &Path,
    temporary_file: &File,
    destination: &Path,
    options: &SqliteStoreOptionsV1,
    reset_cleanup_fault: bool,
) -> Result<PublicationOutcomeV1, StoreErrorV1> {
    validate_owned_temporary_file(temporary, temporary_file, false)?;
    match inspect_publication_destination(destination)? {
        DatabasePathStateV1::Existing => return Ok(PublicationOutcomeV1::Existing),
        DatabasePathStateV1::Missing => {}
    }

    let parent = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    match fs::hard_link(temporary, destination) {
        Ok(()) => {
            if let Err(error) = validate_publication_link_pair_v1(temporary, destination) {
                if publication_was_finalized_v1(temporary, temporary_file, destination)? {
                    return Ok(PublicationOutcomeV1::Published);
                }
                return Err(error);
            }
            File::open(parent)
                .map_err(storage_io)?
                .sync_all()
                .map_err(storage_io)?;
            options.trigger_failpoint(
                StoreFailpointV1::PublicationAfterDestinationLinkBeforeTemporaryUnlink,
            )?;
            if reset_cleanup_fault {
                options.trigger_failpoint(
                    StoreFailpointV1::ResetAfterPublicationBeforeResponseAndTemporaryCleanup,
                )?;
            }
            if let Err(error) = remove_temporary_after_publication(temporary, temporary_file) {
                if publication_was_finalized_v1(temporary, temporary_file, destination)? {
                    return Ok(PublicationOutcomeV1::Published);
                }
                return Err(error);
            }
            validate_existing_regular_private_file_v1(destination)?;
            File::open(parent)
                .map_err(storage_io)?
                .sync_all()
                .map_err(storage_io)?;
            Ok(PublicationOutcomeV1::Published)
        }
        Err(error) if error.kind() == ErrorKind::AlreadyExists => {
            match inspect_publication_destination(destination)? {
                DatabasePathStateV1::Existing => Ok(PublicationOutcomeV1::Existing),
                DatabasePathStateV1::Missing => Err(storage_io(error)),
            }
        }
        Err(error) => Err(storage_io(error)),
    }
}

fn remove_temporary_after_publication(path: &Path, file: &File) -> Result<(), StoreErrorV1> {
    validate_owned_temporary_file(path, file, true)?;
    fs::remove_file(path).map_err(storage_io)
}

fn publication_was_finalized_v1(
    temporary: &Path,
    temporary_file: &File,
    destination: &Path,
) -> Result<bool, StoreErrorV1> {
    match fs::symlink_metadata(temporary) {
        Err(error) if error.kind() == ErrorKind::NotFound => {
            let destination_metadata =
                validate_existing_regular_private_file_metadata_v1(destination)?;
            let temporary_metadata = temporary_file.metadata().map_err(storage_io)?;
            #[cfg(unix)]
            if destination_metadata.nlink() != 1
                || destination_metadata.dev() != temporary_metadata.dev()
                || destination_metadata.ino() != temporary_metadata.ino()
            {
                return Ok(false);
            }
            Ok(true)
        }
        Ok(_) => Ok(false),
        Err(error) => Err(storage_io(error)),
    }
}

fn seed_reset_target(
    temporary: &Path,
    temporary_file: &File,
    destination: &Path,
    context: &ResetSeedContextV1<'_>,
    cleanup_armed: &mut bool,
) -> Result<PublicationOutcomeV1, StoreErrorV1> {
    let mut connection = open_or_initialize_v1(
        temporary,
        context.root,
        context.identity,
        context.options,
        context.now,
    )?;
    {
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(storage)?;
        let changed = transaction
            .execute(
                "UPDATE workspace_state SET next_workspace_sequence = 1, updated_at_ms = ?1 \
                 WHERE singleton = 1 AND next_workspace_sequence = 0",
                [sqlite_u64(context.now.get())?],
            )
            .map_err(storage)?;
        if changed != 1 {
            return Err(invariant(StoreInvariantV1::ResetSeed));
        }
        let inserted_job = transaction
            .execute(
                "INSERT INTO jobs (job_id, workspace_sequence, idempotency_key, request_digest, command_name, \
                 canonical_request_json, state, session_id, submitted_at_ms, finished_at_ms, \
                 terminal_response_json) VALUES (?1, 1, ?2, ?3, 'workspace.reset_all', ?4, \
                 'succeeded', NULL, ?5, ?6, ?7)",
                params![
                    context.request.job_id().as_str(),
                    context.request.idempotency_key().as_str(),
                    context.request.request_digest().as_str(),
                    context.canonical_request,
                    sqlite_u64(context.request.submitted_at().get())?,
                    sqlite_u64(context.now.get())?,
                    context.terminal_response,
                ],
            )
            .map_err(storage)?;
        if inserted_job != 1 {
            return Err(invariant(StoreInvariantV1::ResetSeed));
        }
        let inserted_idempotency = transaction
            .execute(
                "INSERT INTO idempotency_records (idempotency_key, request_digest, job_id, scope_kind, \
                 scope_session_id, terminal_response_json, created_at_ms, updated_at_ms) \
                 VALUES (?1, ?2, ?3, 'workspace', NULL, ?4, ?5, ?5)",
                params![
                    context.request.idempotency_key().as_str(),
                    context.request.request_digest().as_str(),
                    context.request.job_id().as_str(),
                    context.terminal_response,
                    sqlite_u64(context.now.get())?,
                ],
            )
            .map_err(storage)?;
        if inserted_idempotency != 1 {
            return Err(invariant(StoreInvariantV1::ResetSeed));
        }
        let inserted_journal = transaction
            .execute(
                "INSERT INTO operational_journal \
                 (recorded_at_ms, level, event_name, workspace_sequence, job_id, summary, details_json) \
                 VALUES (?1, 'info', 'workspace.reset_all.seeded', 1, ?2, \
                 'workspace reset target seeded', NULL)",
                params![
                    sqlite_u64(context.now.get())?,
                    context.request.job_id().as_str()
                ],
            )
            .map_err(storage)?;
        if inserted_journal != 1 {
            return Err(invariant(StoreInvariantV1::ResetSeed));
        }
        context
            .options
            .trigger_failpoint(StoreFailpointV1::ResetBeforeSeedCommit)?;
        if context.options.failpoint()
            == Some(StoreFailpointV1::ResetBeforeSeedCommitAndTemporaryCleanup)
        {
            *cleanup_armed = true;
        }
        context
            .options
            .trigger_failpoint(StoreFailpointV1::ResetBeforeSeedCommitAndTemporaryCleanup)?;
        transaction.commit().map_err(storage)?;
    }
    context
        .options
        .trigger_failpoint(StoreFailpointV1::ResetAfterSeedCommitBeforePublication)?;
    checkpoint_close_and_sync(connection, temporary)?;
    match publish_temporary_database_no_clobber(
        temporary,
        temporary_file,
        destination,
        context.options,
        true,
    ) {
        Ok(outcome) => Ok(outcome),
        Err(StoreErrorV1::StorageUnavailableV1 { .. })
            if inspect_publication_destination(destination)? == DatabasePathStateV1::Existing =>
        {
            Ok(PublicationOutcomeV1::Existing)
        }
        Err(error) => Err(error),
    }
}

fn validate_reset_seed_input(
    target_identity: &DurableWorktreeIdentityV1,
    request: &AdmitRequestV1,
    result: &podway_core::DomainResult,
) -> Result<(), StoreErrorV1> {
    if !matches!(request.command(), crate::CommandV1::WorkspaceResetAll)
        || request
            .preconditions()
            .expected_session_revision()
            .is_some()
        || request.preconditions().expected_attempt_id().is_some()
        || request.preconditions().expected_item_id().is_some()
        || request.preconditions().expected_item_revision().is_some()
    {
        return Err(invariant(StoreInvariantV1::ResetSeed));
    }
    match result {
        podway_core::DomainResult::WorkspaceReset {
            workspace_id,
            revision,
        } if workspace_id == target_identity.workspace_uuid() && *revision == RevisionV1::ZERO => {
            Ok(())
        }
        _ => Err(invariant(StoreInvariantV1::ResetSeed)),
    }
}

fn reset_terminal_receipt(
    request: &AdmitRequestV1,
    result: podway_core::DomainResult,
) -> TerminalReceiptV1 {
    TerminalReceiptV1::new(
        JobReceiptV1::new(
            1,
            request.job_id().clone(),
            request.request_digest().clone(),
        ),
        TerminalResultV1::Success(result),
    )
}

fn verify_seeded_reset_target(
    path: &Path,
    context: &ResetSeedContextV1<'_>,
) -> Result<TerminalReceiptV1, StoreErrorV1> {
    inspect_database_snapshot_v1(
        path,
        context.identity,
        context.options,
        IntegrityModeV1::Fast,
        context.now,
        |connection| verify_seeded_reset_snapshot(connection, context),
    )
    .map(|(_, receipt)| receipt)
}

fn verify_seeded_reset_snapshot(
    connection: &Connection,
    context: &ResetSeedContextV1<'_>,
) -> Result<TerminalReceiptV1, StoreErrorV1> {
    let workspace: (String, String, String, String, i64, i64, i64) = connection
        .query_row(
            "SELECT workspace_uuid, git_common_fingerprint, git_worktree_fingerprint, \
             last_validated_root, next_workspace_sequence, created_at_ms, updated_at_ms \
             FROM workspace_state WHERE singleton = 1",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            },
        )
        .map_err(|error| storage_record(error, StoreRecordKindV1::Workspace))?;
    if workspace.0 != context.identity.workspace_uuid().as_str()
        || workspace.1 != context.identity.common_dir_identity().as_str()
        || workspace.2 != context.identity.worktree_admin_identity().as_str()
        || workspace.3 != context.root.as_encoded()
        || workspace.4 != 1
    {
        return Err(corrupt(StoreRecordKindV1::Workspace));
    }
    epoch(workspace.5, StoreRecordKindV1::Workspace)?;
    epoch(workspace.6, StoreRecordKindV1::Workspace)?;

    for table in [
        "procedure_snapshots",
        "task_sessions",
        "stage_progress",
        "attempts",
        "item_slots",
        "blockers",
    ] {
        let count: i64 = connection
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .map_err(|error| storage_record(error, StoreRecordKindV1::Workspace))?;
        if count != 0 {
            return Err(corrupt(StoreRecordKindV1::Workspace));
        }
    }

    let jobs: i64 = connection
        .query_row("SELECT COUNT(*) FROM jobs", [], |row| row.get(0))
        .map_err(|error| storage_record(error, StoreRecordKindV1::Job))?;
    let idempotency_records: i64 = connection
        .query_row("SELECT COUNT(*) FROM idempotency_records", [], |row| {
            row.get(0)
        })
        .map_err(|error| storage_record(error, StoreRecordKindV1::IdempotencyRecord))?;
    let journals: i64 = connection
        .query_row("SELECT COUNT(*) FROM operational_journal", [], |row| {
            row.get(0)
        })
        .map_err(|error| storage_record(error, StoreRecordKindV1::Journal))?;
    if jobs != 1 || idempotency_records != 1 || journals != 1 {
        return Err(corrupt(StoreRecordKindV1::Workspace));
    }

    let job: ResetJobRowV1 = connection
        .query_row(
            "SELECT job_id, workspace_sequence, idempotency_key, request_digest, command_name, \
             canonical_request_json, state, session_id, submitted_at_ms, claimed_at_ms, \
             finished_at_ms, terminal_response_json FROM jobs",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                    row.get(9)?,
                    row.get(10)?,
                    row.get(11)?,
                ))
            },
        )
        .map_err(|error| storage_record(error, StoreRecordKindV1::Job))?;
    if job.0 != context.request.job_id().as_str()
        || job.1 != 1
        || job.2 != context.request.idempotency_key().as_str()
        || job.3 != context.request.request_digest().as_str()
        || job.4 != "workspace.reset_all"
        || job.5 != context.canonical_request
        || job.6 != "succeeded"
        || job.7.is_some()
        || job.8 != sqlite_u64(context.request.submitted_at().get())?
        || job.9.is_some()
        || job.10.is_none()
        || job.11.is_none()
    {
        return Err(corrupt(StoreRecordKindV1::Job));
    }
    epoch(
        job.10.ok_or_else(|| corrupt(StoreRecordKindV1::Job))?,
        StoreRecordKindV1::Job,
    )?;

    let idempotency: (
        String,
        String,
        String,
        Option<String>,
        Option<String>,
        i64,
        i64,
    ) = connection
        .query_row(
            "SELECT request_digest, job_id, scope_kind, scope_session_id, terminal_response_json, \
             created_at_ms, updated_at_ms FROM idempotency_records",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                ))
            },
        )
        .map_err(|error| storage_record(error, StoreRecordKindV1::IdempotencyRecord))?;
    if idempotency.0 != context.request.request_digest().as_str()
        || idempotency.1 != context.request.job_id().as_str()
        || idempotency.2 != "workspace"
        || idempotency.3.is_some()
        || idempotency.4.as_deref() != job.11.as_deref()
    {
        return Err(corrupt(StoreRecordKindV1::IdempotencyRecord));
    }
    epoch(idempotency.5, StoreRecordKindV1::IdempotencyRecord)?;
    epoch(idempotency.6, StoreRecordKindV1::IdempotencyRecord)?;

    let journal: ResetJournalRowV1 = connection
        .query_row(
            "SELECT journal_id, level, event_name, workspace_sequence, job_id, summary, \
                 details_json, recorded_at_ms FROM operational_journal",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                ))
            },
        )
        .map_err(|error| storage_record(error, StoreRecordKindV1::Journal))?;
    if journal.0 != 1
        || journal.1 != "info"
        || journal.2 != "workspace.reset_all.seeded"
        || journal.3 != Some(1)
        || journal.4.as_deref() != Some(context.request.job_id().as_str())
        || journal.5 != "workspace reset target seeded"
        || journal.6.is_some()
    {
        return Err(corrupt(StoreRecordKindV1::Journal));
    }
    epoch(journal.7, StoreRecordKindV1::Journal)?;
    let seed_timestamp = job.10.ok_or_else(|| corrupt(StoreRecordKindV1::Job))?;
    if workspace.5 != seed_timestamp
        || workspace.6 != seed_timestamp
        || idempotency.5 != seed_timestamp
        || idempotency.6 != seed_timestamp
        || journal.7 != seed_timestamp
    {
        return Err(corrupt(StoreRecordKindV1::Workspace));
    }
    let terminal = decode_terminal_receipt_v1(
        job.11
            .as_deref()
            .ok_or_else(|| corrupt(StoreRecordKindV1::Job))?,
    )
    .map_err(|_| corrupt(StoreRecordKindV1::Job))?;
    if terminal.job() != context.receipt.job()
        || terminal.result()
            != &PersistedTerminalResultV1::from_terminal_result(context.receipt.result())
    {
        return Err(corrupt(StoreRecordKindV1::Job));
    }
    verify_seeded_terminal_job_projection(&terminal, "succeeded", job.8, job.9, job.10)?;
    Ok(context.receipt.clone())
}

fn sqlite_sidecar_path(path: &Path, suffix: &str) -> PathBuf {
    let mut sidecar = path.as_os_str().to_os_string();
    sidecar.push(suffix);
    PathBuf::from(sidecar)
}
fn temporary_ownership_marker_path(temporary: &Path) -> PathBuf {
    sqlite_sidecar_path(temporary, ".owner")
}

fn cleanup_temporary_database(
    temporary: &TemporaryDatabaseV1,
    fail_before_database_unlink: bool,
) -> Result<(), StoreErrorV1> {
    validate_database_parent_path_v1(&temporary.database)?;
    if fail_before_database_unlink {
        return Err(failpoint_unavailable());
    }
    remove_owned_temporary_file(&temporary.database, &temporary._database_file, true)?;
    remove_owned_temporary_file(&temporary.wal, &temporary._wal_file, false)?;
    remove_owned_temporary_file(&temporary.shm, &temporary._shm_file, false)?;
    remove_owned_temporary_file(&temporary.marker, &temporary._marker_lock, false)?;
    let parent = temporary
        .database
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    File::open(parent)
        .map_err(storage_io)?
        .sync_all()
        .map_err(storage_io)
}

fn remove_owned_temporary_file(
    path: &Path,
    file: &File,
    allow_publication_link: bool,
) -> Result<(), StoreErrorV1> {
    match fs::symlink_metadata(path) {
        Ok(_) => {
            validate_owned_temporary_file(path, file, allow_publication_link)?;
            fs::remove_file(path).map_err(storage_io)
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(storage_io(error)),
    }
}

fn validate_owned_temporary_file(
    path: &Path,
    file: &File,
    allow_publication_link: bool,
) -> Result<(), StoreErrorV1> {
    let path_metadata = fs::symlink_metadata(path).map_err(storage_io)?;
    if path_metadata.file_type().is_symlink() || !path_metadata.is_file() {
        return Err(StoreErrorV1::StorageUnavailableV1 {
            reason: StoreUnavailableReasonV1::StorageIo,
        });
    }
    #[cfg(unix)]
    {
        let descriptor_metadata = file.metadata().map_err(storage_io)?;
        if path_metadata.mode() & 0o777 != 0o600
            || (path_metadata.nlink() != 1
                && (!allow_publication_link || path_metadata.nlink() != 2))
            || descriptor_metadata.dev() != path_metadata.dev()
            || descriptor_metadata.ino() != path_metadata.ino()
        {
            return Err(StoreErrorV1::StorageUnavailableV1 {
                reason: StoreUnavailableReasonV1::StorageIo,
            });
        }
    }
    Ok(())
}

fn workspace_sequence_upper_bound(transaction: &Transaction<'_>) -> Result<u64, StoreErrorV1> {
    let sequence: i64 = transaction
        .query_row(
            "SELECT next_workspace_sequence FROM workspace_state WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .map_err(|error| storage_record(error, StoreRecordKindV1::Workspace))?;
    u64::try_from(sequence).map_err(|_| corrupt(StoreRecordKindV1::Workspace))
}

fn replay_for_idempotency(
    transaction: &Transaction<'_>,
    stored_job_id: &str,
    stored_digest: &CanonicalRequestDigestV1,
    terminal: Option<String>,
) -> Result<JobReceiptOrTerminalV1, StoreErrorV1> {
    let row: Option<JobStateRow> = transaction
        .query_row(
            "SELECT workspace_sequence, request_digest, state, terminal_response_json, \
             submitted_at_ms, claimed_at_ms, finished_at_ms FROM jobs WHERE job_id = ?1",
            [stored_job_id],
            |row| {
                Ok(JobStateRow {
                    sequence: row.get(0)?,
                    digest: row.get(1)?,
                    state: row.get(2)?,
                    terminal: row.get(3)?,
                    submitted_at: row.get(4)?,
                    claimed_at: row.get(5)?,
                    finished_at: row.get(6)?,
                })
            },
        )
        .optional()
        .map_err(|error| storage_record(error, StoreRecordKindV1::Job))?;
    match row {
        Some(row) => {
            let sequence = u64::try_from(row.sequence)
                .map_err(|_| invariant(StoreInvariantV1::QueueSequence))?;
            if sequence == 0 {
                return Err(corrupt(StoreRecordKindV1::Job));
            }
            let digest = CanonicalRequestDigestV1::new(row.digest)
                .map_err(|_| corrupt(StoreRecordKindV1::Job))?;
            if digest != *stored_digest || row.terminal != terminal {
                return Err(corrupt(StoreRecordKindV1::IdempotencyRecord));
            }
            verify_persisted_job_scope(transaction, stored_job_id)?;
            let receipt = JobReceiptV1::new(sequence, job_id(stored_job_id.to_owned())?, digest);
            match row.state.as_str() {
                "succeeded" | "failed" | "cancelled" => {
                    let terminal =
                        validated_terminal(&receipt, &row.state, row.terminal.as_deref())?;
                    verify_terminal_job_projection(
                        &terminal,
                        &row.state,
                        row.submitted_at,
                        row.claimed_at,
                        row.finished_at,
                    )?;
                    validate_persisted_terminal_for_job(transaction, stored_job_id, &terminal)?;
                    Ok(JobReceiptOrTerminalV1::TerminalReceipt(terminal))
                }
                "queued" | "running" if row.terminal.is_none() => {
                    Ok(JobReceiptOrTerminalV1::JobReceipt(receipt))
                }
                _ => Err(corrupt(StoreRecordKindV1::Job)),
            }
        }
        None => {
            let encoded = terminal.ok_or_else(|| corrupt(StoreRecordKindV1::IdempotencyRecord))?;
            let receipt = decode_terminal_receipt_v1(&encoded)
                .map_err(|_| corrupt(StoreRecordKindV1::IdempotencyRecord))?;
            let max_workspace_sequence = workspace_sequence_upper_bound(transaction)?;
            if receipt.job().identity_sequence() == 0
                || receipt.job().identity_sequence() > max_workspace_sequence
                || receipt.job().job_id().as_str() != stored_job_id
                || receipt.job().request_digest() != stored_digest
            {
                return Err(corrupt(StoreRecordKindV1::IdempotencyRecord));
            }
            Ok(JobReceiptOrTerminalV1::TerminalReceipt(receipt))
        }
    }
}

fn validated_terminal(
    receipt: &JobReceiptV1,
    state: &str,
    encoded: Option<&str>,
) -> Result<PersistedTerminalReceiptV1, StoreErrorV1> {
    let encoded = encoded.ok_or_else(|| corrupt(StoreRecordKindV1::Job))?;
    let terminal =
        decode_terminal_receipt_v1(encoded).map_err(|_| corrupt(StoreRecordKindV1::Job))?;
    if receipt.identity_sequence() == 0 || terminal.job() != receipt {
        return Err(corrupt(StoreRecordKindV1::Job));
    }
    let compatible = matches!(
        (state, terminal.result()),
        ("succeeded", PersistedTerminalResultV1::Success(_))
            | ("failed", PersistedTerminalResultV1::Failure(_))
            | ("cancelled", PersistedTerminalResultV1::Cancelled)
    );
    if !compatible {
        return Err(corrupt(StoreRecordKindV1::Job));
    }
    Ok(terminal)
}
fn verify_terminal_job_projection(
    terminal: &PersistedTerminalReceiptV1,
    state: &str,
    submitted_at: i64,
    claimed_at: Option<i64>,
    finished_at: Option<i64>,
) -> Result<(), StoreErrorV1> {
    let state = match state {
        "succeeded" => PersistedTerminalJobStateV1::Succeeded,
        "failed" => PersistedTerminalJobStateV1::Failed,
        "cancelled" => PersistedTerminalJobStateV1::Cancelled,
        _ => return Err(corrupt(StoreRecordKindV1::Job)),
    };
    let expected = PersistedTerminalJobProjectionV1::new(
        state,
        epoch(submitted_at, StoreRecordKindV1::Job)?,
        claimed_at
            .map(|claimed_at| epoch(claimed_at, StoreRecordKindV1::Job))
            .transpose()?,
        epoch(
            finished_at.ok_or_else(|| corrupt(StoreRecordKindV1::Job))?,
            StoreRecordKindV1::Job,
        )?,
    )
    .map_err(|_| corrupt(StoreRecordKindV1::Job))?;
    if terminal
        .job_projection()
        .is_some_and(|projection| projection != &expected)
    {
        return Err(corrupt(StoreRecordKindV1::Job));
    }
    Ok(())
}

fn verify_seeded_terminal_job_projection(
    terminal: &PersistedTerminalReceiptV1,
    state: &str,
    submitted_at: i64,
    claimed_at: Option<i64>,
    finished_at: Option<i64>,
) -> Result<(), StoreErrorV1> {
    verify_terminal_job_projection(terminal, state, submitted_at, claimed_at, finished_at)?;
    if terminal.job_projection().is_none() {
        return Err(corrupt(StoreRecordKindV1::Job));
    }
    Ok(())
}

fn terminal_session_projection(
    result: &PersistedTerminalResultV1,
    post_transition_session: Option<&podway_core::SessionAggregateV1>,
) -> Result<Option<PersistedTerminalSessionProjectionV1>, StoreErrorV1> {
    let (session_id, revision_before, revision_after) = match result {
        PersistedTerminalResultV1::Success(
            PersistedDomainResultV1::SessionChanged {
                session_id,
                revision_before,
                revision_after,
                ..
            }
            | PersistedDomainResultV1::ItemChanged {
                session_id,
                revision_before,
                revision_after,
                ..
            },
        ) => (session_id, *revision_before, *revision_after),
        PersistedTerminalResultV1::Success(
            PersistedDomainResultV1::WorkspaceInitialized { .. }
            | PersistedDomainResultV1::WorkspaceReset { .. },
        )
        | PersistedTerminalResultV1::Failure(_)
        | PersistedTerminalResultV1::Cancelled => return Ok(None),
    };
    let Some(aggregate) = post_transition_session else {
        return Ok(None);
    };
    if aggregate.session_id() != session_id || aggregate.revision() != revision_after {
        return Err(invariant(StoreInvariantV1::TransitionMutationShape));
    }
    PersistedTerminalSessionProjectionV1::new(
        aggregate.session_id().clone(),
        aggregate.task_title().to_owned(),
        aggregate.lifecycle().into(),
        revision_before,
        revision_after,
    )
    .map(Some)
    .map_err(|_| corrupt(StoreRecordKindV1::Session))
}
fn validate_success_transition(
    command: &crate::CommandV1,
    result: &podway_core::DomainResult,
    transition: &StateTransitionV1,
    current: Option<&podway_core::SessionAggregateV1>,
    identity: &DurableWorktreeIdentityV1,
) -> Result<(), StoreErrorV1> {
    if let PersistedSessionMutationV1::ReplaceFresh(aggregate) =
        transition.persisted_session_mutation()
    {
        let matches = match result {
            podway_core::DomainResult::SessionChanged {
                session_id,
                revision_before,
                revision_after,
                changed,
            } => {
                matches!(command, crate::CommandV1::SessionStartReplace)
                    && *changed
                    && transition.session_id() == Some(session_id)
                    && aggregate.session_id() == session_id
                    && transition.resulting_workspace_revision() == RevisionV1::new(1)
                    && aggregate.revision() == RevisionV1::new(1)
                    && *revision_after == RevisionV1::new(1)
                    && current.is_some_and(|session| {
                        session.session_id() != session_id
                            && transition.previous_workspace_revision() == session.revision()
                            && *revision_before == session.revision()
                    })
            }
            podway_core::DomainResult::WorkspaceInitialized { .. }
            | podway_core::DomainResult::WorkspaceReset { .. }
            | podway_core::DomainResult::ItemChanged { .. } => false,
        };
        return matches
            .then_some(())
            .ok_or_else(|| invariant(StoreInvariantV1::TransitionMutationShape));
    }

    let session_result_matches =
        |session_id: &podway_core::SessionId| match transition.persisted_session_mutation() {
            PersistedSessionMutationV1::Unchanged => {
                transition.session_id() == Some(session_id)
                    && current.is_some_and(|session| session.session_id() == session_id)
            }
            PersistedSessionMutationV1::Replace(aggregate) => {
                transition.session_id() == Some(session_id)
                    && aggregate.session_id() == session_id
                    && current.is_none_or(|session| session.session_id() == session_id)
            }
            PersistedSessionMutationV1::ReplaceFresh(_) => false,
            PersistedSessionMutationV1::Clear => {
                transition.session_id().is_none()
                    && current.is_some_and(|session| session.session_id() == session_id)
            }
        };
    let matches = match result {
        podway_core::DomainResult::WorkspaceInitialized {
            workspace_id,
            revision,
        } => {
            matches!(command, crate::CommandV1::WorkspaceInitialize)
                && workspace_id == identity.workspace_uuid()
                && *revision == transition.resulting_workspace_revision()
                && transition.previous_workspace_revision() == RevisionV1::ZERO
                && transition.session_id().is_none()
                && matches!(
                    transition.persisted_session_mutation(),
                    PersistedSessionMutationV1::Unchanged
                )
                && current.is_none()
        }
        podway_core::DomainResult::WorkspaceReset { .. } => false,
        podway_core::DomainResult::SessionChanged {
            session_id,
            revision_before,
            revision_after,
            ..
        } => {
            transition.previous_workspace_revision() == *revision_before
                && transition.resulting_workspace_revision() == *revision_after
                && session_result_matches(session_id)
        }
        podway_core::DomainResult::ItemChanged {
            session_id,
            revision_before,
            revision_after,
            ..
        } => {
            transition.previous_workspace_revision() == *revision_before
                && transition.resulting_workspace_revision() == *revision_after
                && session_result_matches(session_id)
        }
    };
    matches
        .then_some(())
        .ok_or_else(|| invariant(StoreInvariantV1::TransitionMutationShape))
}

fn validate_preconditions(
    preconditions: &crate::RevisionAttemptItemPreconditionsV1,
    current: Option<&podway_core::SessionAggregateV1>,
) -> Result<(), StoreErrorV1> {
    let actual_session = current.map(podway_core::SessionAggregateV1::revision);
    if preconditions.expected_session_revision() != actual_session
        && preconditions.expected_session_revision().is_some()
    {
        return Err(StoreErrorV1::PreconditionConflictV1 {
            expected: preconditions.expected_session_revision(),
            actual: actual_session,
        });
    }
    match preconditions.expected_attempt_id() {
        Some(expected_attempt)
            if current.and_then(podway_core::SessionAggregateV1::active_attempt_id)
                != Some(expected_attempt) =>
        {
            return Err(StoreErrorV1::PreconditionConflictV1 {
                expected: preconditions.expected_session_revision(),
                actual: actual_session,
            });
        }
        _ => {}
    }
    if let Some(item_id) = preconditions.expected_item_id() {
        let actual = current
            .and_then(|session| {
                session.active_attempt_id().and_then(|attempt_id| {
                    session
                        .attempts()
                        .iter()
                        .find(|attempt| attempt.attempt_id() == attempt_id)
                })
            })
            .and_then(|attempt| {
                attempt
                    .item_slots()
                    .iter()
                    .find(|slot| slot.item_id() == item_id)
            })
            .map(podway_core::ItemSlotV1::revision);
        if actual.is_none()
            || preconditions
                .expected_item_revision()
                .is_some_and(|expected| Some(expected) != actual)
        {
            return Err(StoreErrorV1::PreconditionConflictV1 {
                expected: preconditions.expected_item_revision(),
                actual,
            });
        }
    }
    Ok(())
}
fn admission_session_scope(
    transaction: &Transaction<'_>,
    command: &crate::CommandV1,
    expected_session_revision: Option<RevisionV1>,
) -> Result<Option<String>, StoreErrorV1> {
    if !command_is_session_scoped_v1(command) {
        return Ok(None);
    }
    transaction
        .query_row(
            "SELECT session_id FROM task_sessions WHERE singleton = 1",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| storage_record(error, StoreRecordKindV1::Session))?
        .ok_or(StoreErrorV1::PreconditionConflictV1 {
            expected: expected_session_revision,
            actual: None,
        })
        .map(Some)
}
fn expected_persisted_job_scope<'a>(
    command: &crate::CommandV1,
    session_id: Option<&'a str>,
) -> Result<(&'static str, Option<&'a str>), StoreErrorV1> {
    if command_is_session_scoped_v1(command) {
        let session_id = session_id.ok_or_else(|| corrupt(StoreRecordKindV1::Job))?;
        podway_core::SessionId::new(session_id.to_owned())
            .map_err(|_| corrupt(StoreRecordKindV1::Job))?;
        Ok(("session", Some(session_id)))
    } else if session_id.is_some() {
        Err(corrupt(StoreRecordKindV1::Job))
    } else {
        Ok(("workspace", None))
    }
}

fn persisted_job_execution_v1(
    transaction: &Transaction<'_>,
    job_id: &str,
) -> Result<(ClaimedExecutionV1, Option<String>), StoreErrorV1> {
    let (request, command_name, session_id): (String, String, Option<String>) = transaction
        .query_row(
            "SELECT canonical_request_json, command_name, session_id FROM jobs WHERE job_id = ?1",
            [job_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|error| storage_record(error, StoreRecordKindV1::Job))?
        .ok_or_else(|| corrupt(StoreRecordKindV1::Job))?;
    let execution = decode_command_v1(&request).map_err(|_| corrupt(StoreRecordKindV1::Job))?;
    if command_name != command_name_v1(execution.command()) {
        return Err(corrupt(StoreRecordKindV1::Job));
    }
    Ok((execution, session_id))
}

fn verify_persisted_job_scope(
    transaction: &Transaction<'_>,
    job_id: &str,
) -> Result<(), StoreErrorV1> {
    let (execution, job_session_id) = persisted_job_execution_v1(transaction, job_id)?;
    let expected_scope =
        expected_persisted_job_scope(execution.command(), job_session_id.as_deref())?;
    let (scope_kind, scope_session_id): (String, Option<String>) = transaction
        .query_row(
            "SELECT scope_kind, scope_session_id FROM idempotency_records WHERE job_id = ?1",
            [job_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| storage_record(error, StoreRecordKindV1::IdempotencyRecord))?
        .ok_or_else(|| corrupt(StoreRecordKindV1::IdempotencyRecord))?;
    if (scope_kind.as_str(), scope_session_id.as_deref()) != expected_scope {
        return Err(corrupt(StoreRecordKindV1::IdempotencyRecord));
    }
    Ok(())
}

fn validate_persisted_terminal_for_job(
    transaction: &Transaction<'_>,
    job_id: &str,
    terminal: &PersistedTerminalReceiptV1,
) -> Result<(), StoreErrorV1> {
    let (execution, _) = persisted_job_execution_v1(transaction, job_id)?;
    validate_persisted_terminal_result_for_command_v1(execution.command(), terminal.result())
        .map_err(|_| corrupt(StoreRecordKindV1::Job))
}

fn verify_live_idempotency(
    transaction: &Transaction<'_>,
    job_id: &JobIdV1,
    expected_digest: &CanonicalRequestDigestV1,
) -> Result<(), StoreErrorV1> {
    let count: i64 = transaction
        .query_row(
            "SELECT COUNT(*) FROM idempotency_records WHERE job_id = ?1",
            [job_id.as_str()],
            |row| row.get(0),
        )
        .map_err(|error| storage_record(error, StoreRecordKindV1::IdempotencyRecord))?;
    if count != 1 {
        return Err(corrupt(StoreRecordKindV1::IdempotencyRecord));
    }
    verify_persisted_job_scope(transaction, job_id.as_str())?;
    let (stored_job_id, stored_digest, terminal): (String, String, Option<String>) = transaction
        .query_row(
            "SELECT job_id, request_digest, terminal_response_json \
             FROM idempotency_records WHERE job_id = ?1",
            [job_id.as_str()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .map_err(|error| storage_record(error, StoreRecordKindV1::IdempotencyRecord))?;
    let stored_digest = CanonicalRequestDigestV1::new(stored_digest)
        .map_err(|_| corrupt(StoreRecordKindV1::IdempotencyRecord))?;
    if stored_job_id != job_id.as_str() || stored_digest != *expected_digest || terminal.is_some() {
        return Err(corrupt(StoreRecordKindV1::IdempotencyRecord));
    }
    Ok(())
}
fn verify_terminal_idempotency(
    transaction: &Transaction<'_>,
    job_id: &JobIdV1,
    expected_digest: &CanonicalRequestDigestV1,
    receipt: &PersistedTerminalReceiptV1,
) -> Result<(), StoreErrorV1> {
    let count: i64 = transaction
        .query_row(
            "SELECT COUNT(*) FROM idempotency_records WHERE job_id = ?1",
            [job_id.as_str()],
            |row| row.get(0),
        )
        .map_err(|error| storage_record(error, StoreRecordKindV1::IdempotencyRecord))?;
    if count != 1
        || receipt.job().job_id() != job_id
        || receipt.job().request_digest() != expected_digest
    {
        return Err(corrupt(StoreRecordKindV1::IdempotencyRecord));
    }
    verify_persisted_job_scope(transaction, job_id.as_str())?;
    validate_persisted_terminal_for_job(transaction, job_id.as_str(), receipt)?;
    let (stored_digest, stored_terminal): (String, Option<String>) = transaction
        .query_row(
            "SELECT request_digest, terminal_response_json FROM idempotency_records WHERE job_id = ?1",
            [job_id.as_str()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|error| storage_record(error, StoreRecordKindV1::IdempotencyRecord))?;
    let stored_digest = CanonicalRequestDigestV1::new(stored_digest)
        .map_err(|_| corrupt(StoreRecordKindV1::IdempotencyRecord))?;
    let expected_terminal = encode_persisted_terminal_receipt_v1(receipt)
        .map_err(|_| corrupt(StoreRecordKindV1::IdempotencyRecord))?;
    if stored_digest != *expected_digest
        || stored_terminal.as_deref() != Some(expected_terminal.as_str())
    {
        return Err(corrupt(StoreRecordKindV1::IdempotencyRecord));
    }
    Ok(())
}

fn failpoint_unavailable() -> StoreErrorV1 {
    StoreErrorV1::StorageUnavailableV1 {
        reason: StoreUnavailableReasonV1::Recovery,
    }
}

fn is_session_barrier(command: &crate::CommandV1) -> bool {
    matches!(
        command,
        crate::CommandV1::SessionReset | crate::CommandV1::SessionStartReplace
    )
}
fn terminal_state(state: &str) -> bool {
    matches!(state, "succeeded" | "failed" | "cancelled")
}

struct JobViewRowV1 {
    job_id: String,
    sequence: i64,
    digest: String,
    command_name: String,
    request: String,
    state: String,
    submitted_at: i64,
    claimed_at: Option<i64>,
    finished_at: Option<i64>,
    terminal: Option<String>,
}

impl JobViewRowV1 {
    fn from_row(row: &Row<'_>) -> rusqlite::Result<Self> {
        let _session_id: Option<String> = row.get(6)?;
        Ok(Self {
            job_id: row.get(0)?,
            sequence: row.get(1)?,
            digest: row.get(2)?,
            command_name: row.get(3)?,
            request: row.get(4)?,
            state: row.get(5)?,
            submitted_at: row.get(7)?,
            claimed_at: row.get(8)?,
            finished_at: row.get(9)?,
            terminal: row.get(10)?,
        })
    }
}

fn decode_job_view_v1(
    transaction: &Transaction<'_>,
    row: JobViewRowV1,
) -> Result<JobViewV1, StoreErrorV1> {
    let sequence =
        u64::try_from(row.sequence).map_err(|_| invariant(StoreInvariantV1::QueueSequence))?;
    if sequence == 0 {
        return Err(corrupt(StoreRecordKindV1::Job));
    }
    let job_id = job_id(row.job_id)?;
    let digest = digest(row.digest)?;
    let job = JobReceiptV1::new(sequence, job_id, digest);
    let execution = decode_command_v1(&row.request).map_err(|_| corrupt(StoreRecordKindV1::Job))?;
    if command_name_v1(execution.command()) != row.command_name {
        return Err(corrupt(StoreRecordKindV1::Job));
    }
    verify_persisted_job_scope(transaction, job.job_id().as_str())?;

    let state = match row.state.as_str() {
        "queued" => JobStateV1::Queued,
        "running" => JobStateV1::Running,
        "succeeded" => JobStateV1::Succeeded,
        "failed" => JobStateV1::Failed,
        "cancelled" => JobStateV1::Cancelled,
        _ => return Err(corrupt(StoreRecordKindV1::Job)),
    };
    let submitted_at = epoch(row.submitted_at, StoreRecordKindV1::Job)?;
    let claimed_at = row
        .claimed_at
        .map(|value| epoch(value, StoreRecordKindV1::Job))
        .transpose()?;
    let finished_at = row
        .finished_at
        .map(|value| epoch(value, StoreRecordKindV1::Job))
        .transpose()?;

    let terminal_receipt = match state {
        JobStateV1::Queued => {
            if claimed_at.is_some() || finished_at.is_some() || row.terminal.is_some() {
                return Err(corrupt(StoreRecordKindV1::Job));
            }
            None
        }
        JobStateV1::Running => {
            if claimed_at.is_none() || finished_at.is_some() || row.terminal.is_some() {
                return Err(corrupt(StoreRecordKindV1::Job));
            }
            None
        }
        JobStateV1::Succeeded | JobStateV1::Failed | JobStateV1::Cancelled => {
            if finished_at.is_none() {
                return Err(corrupt(StoreRecordKindV1::Job));
            }
            let terminal = validated_terminal(&job, &row.state, row.terminal.as_deref())?;
            verify_terminal_job_projection(
                &terminal,
                &row.state,
                row.submitted_at,
                row.claimed_at,
                row.finished_at,
            )?;
            validate_persisted_terminal_result_for_command_v1(
                execution.command(),
                terminal.result(),
            )
            .map_err(|_| corrupt(StoreRecordKindV1::Job))?;
            Some(terminal)
        }
    };

    Ok(JobViewV1::new(
        execution,
        job,
        state,
        submitted_at,
        claimed_at,
        finished_at,
        terminal_receipt,
    ))
}
fn job_id(value: String) -> Result<JobIdV1, StoreErrorV1> {
    JobIdV1::new(value).map_err(|_| corrupt(StoreRecordKindV1::Job))
}
fn digest(value: String) -> Result<CanonicalRequestDigestV1, StoreErrorV1> {
    CanonicalRequestDigestV1::new(value).map_err(|_| corrupt(StoreRecordKindV1::Job))
}
fn epoch(value: i64, record: StoreRecordKindV1) -> Result<EpochMillisV1, StoreErrorV1> {
    u64::try_from(value)
        .map(EpochMillisV1::new)
        .map_err(|_| corrupt(record))
}
fn sqlite_u64(value: u64) -> Result<i64, StoreErrorV1> {
    i64::try_from(value).map_err(|_| {
        StoreErrorV1::InvalidStateV1(crate::StoreValueErrorV1::IntegerOutOfRange {
            field: "SQLite integer",
        })
    })
}
fn corrupt(record: StoreRecordKindV1) -> StoreErrorV1 {
    StoreErrorV1::CorruptStateV1 { record }
}
fn invariant(invariant: StoreInvariantV1) -> StoreErrorV1 {
    StoreErrorV1::InternalInvariantViolationV1 { invariant }
}
fn storage(error: rusqlite::Error) -> StoreErrorV1 {
    map_rusqlite_error_v1(
        error,
        RusqliteErrorContextV1::Integrity(StoreIntegrityCheckV1::SqliteQuickCheck),
    )
}
fn storage_record(error: rusqlite::Error, record: StoreRecordKindV1) -> StoreErrorV1 {
    map_rusqlite_error_v1(error, RusqliteErrorContextV1::Record(record))
}
fn storage_io(_error: std::io::Error) -> StoreErrorV1 {
    StoreErrorV1::StorageUnavailableV1 {
        reason: StoreUnavailableReasonV1::StorageIo,
    }
}

struct JobRow {
    job_id: String,
    sequence: i64,
    digest: String,
    command_name: String,
    request: String,
}
struct JobStateRow {
    sequence: i64,
    digest: String,
    state: String,
    terminal: Option<String>,
    submitted_at: i64,
    claimed_at: Option<i64>,
    finished_at: Option<i64>,
}
struct RunningJobRow {
    sequence: i64,
    digest: String,
    command_name: String,
    request: String,
    submitted_at: i64,
    claimed_at: Option<i64>,
    state: String,
}
