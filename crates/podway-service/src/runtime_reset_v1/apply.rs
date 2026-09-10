use std::{ffi::OsStr, os::unix::ffi::OsStrExt, path::Path};

use nix::fcntl::{Flock, FlockArg};
use podway_core::{RuntimeModeV1, UnixMillis};
use sha2::{Digest, Sha256};

use super::{
    RuntimeResetErrorV1 as Error, RuntimeResetInspectorV1, RuntimeResetLockV1,
    RuntimeResetOperationV1, RuntimeResetPlannerV1, RuntimeResetProcessV1, RuntimeResetReasonV1,
    RuntimeResetSelectionV1,
    document::{
        EntryKind, EntryState, ModePhase, RecordMode, RecordPhase, ResetRecord, ResetToken,
        RuntimeResetExclusionV1, RuntimeResetModeOutcomeStateV1, RuntimeResetModeOutcomeV1,
        RuntimeResetPathV1, RuntimeResetResourceClassV1, RuntimeResetResourceOutcomeStateV1,
        RuntimeResetResourceOutcomeV1, RuntimeResetResultStatusV1, RuntimeResetResultV1,
        decode_bytes, encode_bytes,
    },
    filesystem::{self, Directory, FileKind},
};
use crate::{
    LaunchctlRunnerV1, PodwayHomeV1, ServiceLifecycleTransactionLockV1, ServiceOperationV1,
    ServiceRuntimePathsV1,
};

/// Proof that reset owns the production service transaction lock in canonical order.
pub struct RuntimeResetServiceLifecycleV1 {
    _guard: ServiceLifecycleTransactionLockV1,
}

impl RuntimeResetServiceLifecycleV1 {
    pub fn acquire() -> Result<Self, Error> {
        ServiceLifecycleTransactionLockV1::acquire(ServiceOperationV1::Uninstall)
            .map(|guard| Self { _guard: guard })
            .map_err(|_| Error::unsafe_reason(RuntimeResetReasonV1::Io))
    }
}

/// Runtime/process side effects supplied by the CLI composition boundary.
/// Implementations must not delete runtime data; descriptor-relative deletion remains here.
pub trait RuntimeResetOperatorV1 {
    fn prepare(&mut self, _paths: &ServiceRuntimePathsV1) -> Result<(), Error> {
        Ok(())
    }

    fn reserve(
        &mut self,
        paths: &ServiceRuntimePathsV1,
        operation: &RuntimeResetOperationV1,
        process: &RuntimeResetProcessV1,
    ) -> Result<String, Error>;

    fn snapshot(
        &mut self,
        paths: &ServiceRuntimePathsV1,
        operation: &RuntimeResetOperationV1,
        process: &RuntimeResetProcessV1,
        reservation_id: &str,
    ) -> Result<(), Error>;

    fn stop(
        &mut self,
        paths: &ServiceRuntimePathsV1,
        operation: &RuntimeResetOperationV1,
        process: Option<&RuntimeResetProcessV1>,
        reservation_id: Option<&str>,
        production_service_loaded: bool,
        process_already_stopped: bool,
    ) -> Result<(), Error>;

    fn wait_stopped(
        &mut self,
        paths: &ServiceRuntimePathsV1,
        process: Option<&RuntimeResetProcessV1>,
    ) -> Result<(), Error>;

    fn release_uncommitted(
        &mut self,
        paths: &ServiceRuntimePathsV1,
        operation: &RuntimeResetOperationV1,
        process: &RuntimeResetProcessV1,
        reservation_id: &str,
    );

    /// Test adapters use this to inject failure after durable boundaries.
    fn checkpoint(&mut self, _boundary: &'static str) -> Result<(), Error> {
        Ok(())
    }
}

/// Applies an explicitly selected ordinary runtime set and resumes its exact durable operation.
pub struct RuntimeResetApplyV1<L, I, O> {
    home: PodwayHomeV1,
    planner: RuntimeResetPlannerV1<L, I>,
    operator: O,
}

impl<L: LaunchctlRunnerV1, I: RuntimeResetInspectorV1, O: RuntimeResetOperatorV1>
    RuntimeResetApplyV1<L, I, O>
{
    pub fn new(home: PodwayHomeV1, launchctl: L, inspector: I, operator: O) -> Self {
        Self {
            planner: RuntimeResetPlannerV1::new(home.clone(), launchctl, inspector),
            home,
            operator,
        }
    }

    pub fn apply(
        &mut self,
        encoded_token: &str,
        selection: RuntimeResetSelectionV1,
        now: UnixMillis,
    ) -> Result<RuntimeResetResultV1, Error> {
        let token_sha256 = format!("sha256:{:x}", Sha256::digest(encoded_token.as_bytes()));
        let token = ResetToken::decode(encoded_token)?;
        if token.selection != selection {
            return Err(Error::stale(RuntimeResetReasonV1::SelectionChanged));
        }
        let _reset = RuntimeResetLockV1::acquire_reset(&self.home)?;
        for target in &token.targets {
            let paths = self.paths(&target.mode)?;
            self.operator
                .prepare(&paths)
                .map_err(|error| error.in_mode(&target.mode))?;
        }
        let _topology = RuntimeResetLockV1::acquire_topology(&self.home)?;
        let account = Directory::account(&self.home)?;
        let root = account
            .child_optional(OsStr::new(".podway"), true)?
            .ok_or_else(Error::unsafe_path)?;
        let maintenance = root.create_child(OsStr::new("maintenance"))?;

        if let Some(bytes) = maintenance.read_optional(
            OsStr::new("runtime-reset.json"),
            super::MAX_RUNTIME_RESET_DOCUMENT_BYTES_V1,
        )? {
            let record = ResetRecord::decode_for_apply(&bytes, &self.home)?;
            if record.phase == RecordPhase::Completed {
                if record.token_sha256 == token_sha256 {
                    let mut result = record.result.expect("validated completed record");
                    result.status = RuntimeResetResultStatusV1::AlreadyApplied;
                    return Ok(result);
                }
            } else {
                if record.token_sha256 != token_sha256 {
                    return Err(Error::in_progress());
                }
                if record.plan.as_ref().expect("validated record").selection != selection {
                    return Err(Error::stale(RuntimeResetReasonV1::SelectionChanged));
                }
                return match self.resume(&root, &maintenance, record, true) {
                    Ok(result) => Ok(result),
                    Err(error) => Err(self.committed_error(error, &maintenance, None)),
                };
            }
        }

        self.planner
            .validate_precommit_token(encoded_token, &selection, now)?;
        let operation = RuntimeResetOperationV1 {
            operation_id: uuid::Uuid::new_v4().to_string(),
            token_sha256,
        };
        let mut reservations = Vec::with_capacity(token.targets.len());
        for target in &token.targets {
            let paths = self.paths(&target.mode)?;
            let reservation = match &target.process {
                Some(process) => match self.operator.reserve(&paths, &operation, process) {
                    Ok(reservation) => Some(reservation),
                    Err(error) => {
                        self.release_reservations(&token.targets, &reservations, &operation);
                        return Err(error.in_mode(&target.mode));
                    }
                },
                None => None,
            };
            reservations.push(reservation);
        }

        let commit = match RuntimeResetLockV1::acquire_commit(&self.home) {
            Ok(commit) => commit,
            Err(error) => {
                self.release_reservations(&token.targets, &reservations, &operation);
                return Err(error);
            }
        };
        let commit_result = (|| {
            for (target, reservation) in token.targets.iter().zip(&reservations) {
                if let (Some(process), Some(reservation)) = (&target.process, reservation) {
                    let paths = self.paths(&target.mode)?;
                    self.operator
                        .snapshot(&paths, &operation, process, reservation)
                        .map_err(|error| error.in_mode(&target.mode))?;
                }
            }
            let record = ResetRecord {
                schema: "podway.runtime-reset-record/v1".to_owned(),
                operation_id: operation.operation_id.clone(),
                token_sha256: operation.token_sha256.clone(),
                phase: RecordPhase::InProgress,
                plan: Some(token.clone()),
                modes: reservations
                    .iter()
                    .zip(token.targets.iter())
                    .map(|(reservation, target)| RecordMode {
                        mode: target.mode.clone(),
                        phase: ModePhase::Pending,
                        reservation_id: reservation.clone(),
                        entries: Vec::new(),
                    })
                    .collect(),
                result: None,
            };
            self.persist(&maintenance, &record)?;
            self.operator.checkpoint("commit")?;
            Ok(record)
        })();
        drop(commit);
        let record = match commit_result {
            Ok(record) => record,
            Err(error) => {
                if let Ok(Some(bytes)) = maintenance.read_optional(
                    OsStr::new("runtime-reset.json"),
                    super::MAX_RUNTIME_RESET_DOCUMENT_BYTES_V1,
                ) && let Ok(record) = ResetRecord::decode_for_apply(&bytes, &self.home)
                    && record.phase == RecordPhase::InProgress
                    && record.token_sha256 == operation.token_sha256
                {
                    return Err(self.committed_error(error, &maintenance, Some(&record)));
                }
                self.release_reservations(&token.targets, &reservations, &operation);
                return Err(error);
            }
        };
        match self.resume(&root, &maintenance, record, false) {
            Ok(result) => Ok(result),
            Err(error) => Err(self.committed_error(error, &maintenance, None)),
        }
    }

    fn release_reservations(
        &mut self,
        targets: &[super::document::NamespaceBinding],
        reservations: &[Option<String>],
        operation: &RuntimeResetOperationV1,
    ) {
        for (target, reservation) in targets.iter().zip(reservations).rev() {
            if let (Some(process), Some(reservation), Ok(paths)) = (
                &target.process,
                reservation.as_deref(),
                self.paths(&target.mode),
            ) {
                self.operator
                    .release_uncommitted(&paths, operation, process, reservation);
            }
        }
    }

    fn resume(
        &mut self,
        root: &Directory,
        maintenance: &Directory,
        mut record: ResetRecord,
        reconnect: bool,
    ) -> Result<RuntimeResetResultV1, Error> {
        let operation = RuntimeResetOperationV1 {
            operation_id: record.operation_id.clone(),
            token_sha256: record.token_sha256.clone(),
        };
        let targets = record
            .plan
            .as_ref()
            .expect("validated record")
            .targets
            .clone();
        let mut already_stopped = vec![false; targets.len()];
        if reconnect {
            for (mode_index, (target, mode)) in targets.iter().zip(&record.modes).enumerate() {
                if matches!(mode.phase, ModePhase::Pending | ModePhase::StopIntended)
                    && let Some(process) = &target.process
                {
                    let paths = self.paths(&target.mode)?;
                    let expected = mode.reservation_id.as_deref().ok_or_else(|| {
                        Error::unsafe_reason(RuntimeResetReasonV1::ReservationLost)
                            .in_mode(&target.mode)
                    })?;
                    match self.operator.reserve(&paths, &operation, process) {
                        Ok(reservation) if reservation == expected => {}
                        Ok(_) => {
                            return Err(Error::unsafe_reason(
                                RuntimeResetReasonV1::ReservationLost,
                            )
                            .in_mode(&target.mode));
                        }
                        Err(error) if mode.phase == ModePhase::StopIntended => {
                            if self.operator.wait_stopped(&paths, Some(process)).is_err() {
                                return Err(error.in_mode(&target.mode));
                            }
                            self.verify_singleton(root, target)
                                .map_err(|error| error.in_mode(&target.mode))?;
                            already_stopped[mode_index] = true;
                        }
                        Err(error) => return Err(error.in_mode(&target.mode)),
                    }
                }
            }
        }

        for (mode_index, target) in targets.iter().enumerate() {
            if record.modes[mode_index].phase == ModePhase::Complete {
                continue;
            }
            if record.modes[mode_index].phase == ModePhase::Pending && target.is_absent() {
                record.modes[mode_index].phase = ModePhase::Complete;
                self.persist(maintenance, &record)?;
                continue;
            }
            let paths = self.paths(&target.mode)?;
            if record.modes[mode_index].phase == ModePhase::Pending {
                record.modes[mode_index].phase = ModePhase::StopIntended;
                self.persist(maintenance, &record)?;
                self.operator
                    .checkpoint("stop_intended")
                    .map_err(|error| error.in_mode(&target.mode))?;
            }
            if record.modes[mode_index].phase == ModePhase::StopIntended {
                self.operator
                    .stop(
                        &paths,
                        &operation,
                        target.process.as_ref(),
                        record.modes[mode_index].reservation_id.as_deref(),
                        target.service.loaded,
                        already_stopped[mode_index],
                    )
                    .map_err(|error| error.in_mode(&target.mode))?;
                self.operator
                    .checkpoint("stop_effect")
                    .map_err(|error| error.in_mode(&target.mode))?;
                self.operator
                    .wait_stopped(&paths, target.process.as_ref())
                    .map_err(|error| error.in_mode(&target.mode))?;
                self.verify_singleton(root, target)
                    .map_err(|error| error.in_mode(&target.mode))?;
                record.modes[mode_index].phase = ModePhase::Stopped;
                self.persist(maintenance, &record)?;
                self.operator
                    .checkpoint("stopped")
                    .map_err(|error| error.in_mode(&target.mode))?;
            }
            if record.modes[mode_index].phase == ModePhase::Stopped {
                record.modes[mode_index].phase = ModePhase::InventoryIntended;
                self.persist(maintenance, &record)?;
                self.operator
                    .checkpoint("inventory_intended")
                    .map_err(|error| error.in_mode(&target.mode))?;
            }
            if record.modes[mode_index].phase == ModePhase::InventoryIntended {
                record.modes[mode_index].entries = self
                    .inventory(root, target)
                    .map_err(|error| error.in_mode(&target.mode))?;
                record.modes[mode_index].phase = ModePhase::InventoryReady;
                self.persist(maintenance, &record)?;
                self.operator
                    .checkpoint("inventory_ready")
                    .map_err(|error| error.in_mode(&target.mode))?;
            }

            for entry_index in 0..record.modes[mode_index].entries.len() {
                if record.modes[mode_index].entries[entry_index].state == EntryState::Completed {
                    continue;
                }
                let had_intent =
                    record.modes[mode_index].entries[entry_index].state == EntryState::Intended;
                if !had_intent {
                    record.modes[mode_index].entries[entry_index].state = EntryState::Intended;
                    self.persist(maintenance, &record)?;
                    self.operator
                        .checkpoint("unlink_intended")
                        .map_err(|error| error.in_mode(&target.mode))?;
                }
                let removed = self
                    .remove_entry(root, target, &record.modes[mode_index].entries[entry_index])
                    .map_err(|error| error.in_mode(&target.mode))?;
                if !removed && !had_intent {
                    return Err(Error::unsafe_reason(RuntimeResetReasonV1::ResourceChanged)
                        .in_mode(&target.mode));
                }
                self.operator
                    .checkpoint("unlinked")
                    .map_err(|error| error.in_mode(&target.mode))?;
                record.modes[mode_index].entries[entry_index].state = EntryState::Completed;
                self.persist(maintenance, &record)?;
                self.operator
                    .checkpoint("unlink_completed")
                    .map_err(|error| error.in_mode(&target.mode))?;
            }
            record.modes[mode_index].phase = ModePhase::Complete;
            self.persist(maintenance, &record)?;
        }

        let result = self.result(&record)?;
        let completed = ResetRecord {
            schema: record.schema,
            operation_id: record.operation_id,
            token_sha256: record.token_sha256,
            phase: RecordPhase::Completed,
            plan: None,
            modes: Vec::new(),
            result: Some(result.clone()),
        };
        self.persist(maintenance, &completed)?;
        self.operator.checkpoint("completed")?;
        Ok(result)
    }

    fn paths(&self, mode: &RuntimeModeV1) -> Result<ServiceRuntimePathsV1, Error> {
        ServiceRuntimePathsV1::for_account_home_mode(
            self.home.account_home(),
            mode.clone(),
            self.home.user_id(),
        )
        .map_err(|_| Error::unsafe_path())
    }

    fn persist(&self, maintenance: &Directory, record: &ResetRecord) -> Result<(), Error> {
        maintenance.write_replace(
            OsStr::new("runtime-reset.json"),
            &record.encode(&self.home)?,
        )
    }

    fn namespace(&self, root: &Directory, mode: &RuntimeModeV1) -> Result<Directory, Error> {
        if mode.is_production() {
            root.duplicate()
        } else {
            root.child_optional(OsStr::new("modes"), true)?
                .ok_or_else(Error::unsafe_path)?
                .child_optional(OsStr::new(mode.as_str()), true)?
                .ok_or_else(Error::unsafe_path)
        }
    }

    fn verify_singleton(
        &self,
        root: &Directory,
        target: &super::document::NamespaceBinding,
    ) -> Result<(), Error> {
        let namespace = self.namespace(root, &target.mode)?;
        let expected = target
            .root_identity
            .as_ref()
            .ok_or_else(Error::unsafe_path)?;
        if !namespace.identity()?.same_directory_anchor(expected) {
            return Err(Error::unsafe_reason(RuntimeResetReasonV1::IdentityChanged));
        }
        let run = namespace
            .child_optional(OsStr::new("run"), true)?
            .ok_or_else(Error::unsafe_path)?;
        if !run.identity()?.same_directory_anchor(
            target
                .run_identity
                .as_ref()
                .ok_or_else(Error::unsafe_path)?,
        ) {
            return Err(Error::unsafe_reason(RuntimeResetReasonV1::IdentityChanged));
        }
        let current_lock = run.regular_optional(OsStr::new("podwayd.lock"))?;
        if current_lock.as_ref() != target.lock_identity.as_ref() {
            return Err(Error::unsafe_reason(RuntimeResetReasonV1::IdentityChanged));
        }
        if run.regular_optional(OsStr::new("podwayd.lock"))?.as_ref()
            != target.lock_identity.as_ref()
        {
            return Err(Error::unsafe_reason(RuntimeResetReasonV1::IdentityChanged));
        }
        let lock_file = run.open_regular(OsStr::new("podwayd.lock"))?;
        match Flock::lock(lock_file, FlockArg::LockExclusiveNonblock) {
            Ok(lock) => {
                drop(lock);
                Ok(())
            }
            Err(_) => Err(Error::unsafe_reason(RuntimeResetReasonV1::LockHeld)),
        }
    }

    fn inventory(
        &self,
        root: &Directory,
        target: &super::document::NamespaceBinding,
    ) -> Result<Vec<super::document::RecordEntry>, Error> {
        self.verify_singleton(root, target)?;
        let namespace = self.namespace(root, &target.mode)?;
        let mut entries = Vec::new();
        if let Some(run) = namespace.child_optional(OsStr::new("run"), true)? {
            for name in run.entries(super::MAX_RUNTIME_RESET_RESOURCES_V1)? {
                if name == "podwayd.lock" {
                    continue;
                }
                if name != "podwayd.sock" {
                    return Err(Error::unsafe_reason(RuntimeResetReasonV1::UnexpectedEntry));
                }
                if let Some(identity) = run.identity_optional(&name, FileKind::Socket)? {
                    entries.push(super::document::RecordEntry {
                        class: RuntimeResetResourceClassV1::Socket,
                        relative_path_bytes_base64url: encode_bytes(b"run/podwayd.sock"),
                        identity,
                        kind: EntryKind::Socket,
                        state: EntryState::Pending,
                    });
                }
            }
        }
        for (directory_name, directory_class, classifier) in [
            (
                "state",
                RuntimeResetResourceClassV1::StateDirectory,
                filesystem::state_resource_class
                    as fn(&OsStr) -> Option<RuntimeResetResourceClassV1>,
            ),
            (
                "logs",
                RuntimeResetResourceClassV1::LogsDirectory,
                filesystem::log_resource_class,
            ),
        ] {
            let Some(directory) = namespace.child_optional(OsStr::new(directory_name), true)?
            else {
                continue;
            };
            let expected = if directory_name == "state" {
                target.state_identity.as_ref()
            } else {
                target.logs_identity.as_ref()
            };
            if !expected.is_some_and(|identity| {
                directory
                    .identity()
                    .is_ok_and(|current| current.same_directory_anchor(identity))
            }) {
                return Err(Error::unsafe_reason(RuntimeResetReasonV1::IdentityChanged));
            }
            let mut retained = false;
            for name in directory.entries(super::MAX_RUNTIME_RESET_RESOURCES_V1)? {
                if directory_name == "state" && name == "workspaces.json.lock" {
                    directory.regular_optional(&name)?;
                    retained = true;
                    continue;
                }
                let class = classifier(&name)
                    .ok_or_else(|| Error::unsafe_reason(RuntimeResetReasonV1::UnexpectedEntry))?;
                let identity = directory
                    .regular_optional(&name)?
                    .ok_or_else(Error::unsafe_path)?;
                let mut relative = directory_name.as_bytes().to_vec();
                relative.push(b'/');
                relative.extend_from_slice(name.as_os_str().as_bytes());
                entries.push(super::document::RecordEntry {
                    class,
                    relative_path_bytes_base64url: encode_bytes(&relative),
                    identity,
                    kind: EntryKind::Regular,
                    state: EntryState::Pending,
                });
            }
            if !retained {
                entries.push(super::document::RecordEntry {
                    class: directory_class,
                    relative_path_bytes_base64url: encode_bytes(directory_name.as_bytes()),
                    identity: directory.identity()?,
                    kind: EntryKind::Directory,
                    state: EntryState::Pending,
                });
            }
        }
        if entries.len() > super::MAX_RUNTIME_RESET_RESOURCES_V1 {
            return Err(Error::limit());
        }
        // Leaves precede their containing directories; all other order is byte-stable.
        entries.sort_by(|left, right| {
            (
                left.kind == EntryKind::Directory,
                &left.relative_path_bytes_base64url,
            )
                .cmp(&(
                    right.kind == EntryKind::Directory,
                    &right.relative_path_bytes_base64url,
                ))
        });
        Ok(entries)
    }

    fn remove_entry(
        &self,
        root: &Directory,
        target: &super::document::NamespaceBinding,
        entry: &super::document::RecordEntry,
    ) -> Result<bool, Error> {
        let relative = decode_bytes(&entry.relative_path_bytes_base64url)?;
        let path = Path::new(OsStr::from_bytes(&relative));
        let namespace = self.namespace(root, &target.mode)?;
        let name = path.file_name().ok_or_else(Error::unsafe_path)?;
        let parent = match path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            None => namespace,
            Some(parent)
                if parent == Path::new("run")
                    || parent == Path::new("state")
                    || parent == Path::new("logs") =>
            {
                namespace
                    .child_optional(parent.as_os_str(), true)?
                    .ok_or_else(Error::unsafe_path)?
            }
            _ => return Err(Error::unsafe_path()),
        };
        match entry.kind {
            EntryKind::Regular => parent.remove_expected(name, &entry.identity, FileKind::Regular),
            EntryKind::Socket => parent.remove_expected(name, &entry.identity, FileKind::Socket),
            EntryKind::Directory => parent.remove_expected_directory(name, &entry.identity),
        }
    }

    fn result(&self, record: &ResetRecord) -> Result<RuntimeResetResultV1, Error> {
        self.result_with_status(record, RuntimeResetResultStatusV1::Complete)
    }

    fn committed_error(
        &self,
        error: Error,
        maintenance: &Directory,
        fallback: Option<&ResetRecord>,
    ) -> Error {
        let failed_mode = error.mode.clone();
        let reason = error.reason;
        let current = maintenance
            .read_optional(
                OsStr::new("runtime-reset.json"),
                super::MAX_RUNTIME_RESET_DOCUMENT_BYTES_V1,
            )
            .ok()
            .flatten()
            .and_then(|bytes| ResetRecord::decode_for_apply(&bytes, &self.home).ok());
        let Some(record) = current.as_ref().or(fallback) else {
            return error;
        };
        if record.phase == RecordPhase::Completed {
            if let Some(mut result) = record.result.clone() {
                result.status = RuntimeResetResultStatusV1::Incomplete;
                Self::mark_incomplete(&mut result, failed_mode.as_ref());
                return Error::incomplete(reason, result);
            }
            return error;
        }
        match self.result_with_status(record, RuntimeResetResultStatusV1::Incomplete) {
            Ok(mut result) => {
                Self::mark_incomplete(&mut result, failed_mode.as_ref());
                Error::incomplete(reason, result)
            }
            Err(_) => error,
        }
    }

    fn mark_incomplete(result: &mut RuntimeResetResultV1, failed_mode: Option<&RuntimeModeV1>) {
        if result
            .modes
            .iter()
            .any(|mode| mode.state == RuntimeResetModeOutcomeStateV1::Incomplete)
        {
            return;
        }
        let index = failed_mode
            .and_then(|failed| result.modes.iter().position(|mode| mode.mode == *failed))
            .or_else(|| {
                result
                    .modes
                    .iter()
                    .position(|mode| mode.state == RuntimeResetModeOutcomeStateV1::Pending)
            })
            .or_else(|| result.modes.len().checked_sub(1));
        if let Some(index) = index {
            result.modes[index].state = RuntimeResetModeOutcomeStateV1::Incomplete;
        }
    }

    fn result_with_status(
        &self,
        record: &ResetRecord,
        status: RuntimeResetResultStatusV1,
    ) -> Result<RuntimeResetResultV1, Error> {
        let plan = record.plan.as_ref().expect("in-progress record");
        let mut modes = Vec::with_capacity(record.modes.len());
        let mut preserved = Vec::new();
        for (target, mode) in plan.targets.iter().zip(&record.modes) {
            let mut resources = mode
                .entries
                .iter()
                .map(|entry| {
                    let bytes = decode_bytes(&entry.relative_path_bytes_base64url)?;
                    let mut path = Path::new(OsStr::from_bytes(&decode_bytes(
                        &target.root.path_bytes_base64url,
                    )?))
                    .to_path_buf();
                    path.push(OsStr::from_bytes(&bytes));
                    Ok(RuntimeResetResourceOutcomeV1 {
                        class: entry.class,
                        path: RuntimeResetPathV1::new(&path)?,
                        state: if entry.state == EntryState::Completed {
                            RuntimeResetResourceOutcomeStateV1::Removed
                        } else {
                            RuntimeResetResourceOutcomeStateV1::Pending
                        },
                    })
                })
                .collect::<Result<Vec<_>, Error>>()?;
            let stopped = matches!(
                mode.phase,
                ModePhase::Stopped
                    | ModePhase::InventoryIntended
                    | ModePhase::InventoryReady
                    | ModePhase::Complete
            );
            let post_stop_state = if stopped {
                RuntimeResetResourceOutcomeStateV1::AlreadyAbsent
            } else {
                RuntimeResetResourceOutcomeStateV1::Pending
            };
            let paths = self.paths(&target.mode)?;
            if target.socket_identity.is_some()
                && !resources
                    .iter()
                    .any(|resource| resource.class == RuntimeResetResourceClassV1::Socket)
            {
                resources.push(RuntimeResetResourceOutcomeV1 {
                    class: RuntimeResetResourceClassV1::Socket,
                    path: RuntimeResetPathV1::new(paths.socket_path().as_path())?,
                    state: post_stop_state,
                });
            }
            if target.service.metadata_identity.is_some()
                && !resources
                    .iter()
                    .any(|resource| resource.class == RuntimeResetResourceClassV1::ServiceMetadata)
            {
                resources.push(RuntimeResetResourceOutcomeV1 {
                    class: RuntimeResetResourceClassV1::ServiceMetadata,
                    path: RuntimeResetPathV1::new(paths.metadata_index_path().as_path())?,
                    state: post_stop_state,
                });
            }
            if target.service.plist_identity.is_some() {
                resources.push(RuntimeResetResourceOutcomeV1 {
                    class: RuntimeResetResourceClassV1::ServicePlist,
                    path: RuntimeResetPathV1::new(paths.launch_agent_path().as_path())?,
                    state: post_stop_state,
                });
            }
            resources.sort_by(|left, right| {
                left.path
                    .path_bytes_base64url
                    .as_bytes()
                    .cmp(right.path.path_bytes_base64url.as_bytes())
            });
            modes.push(RuntimeResetModeOutcomeV1 {
                mode: target.mode.clone(),
                state: if mode.phase == ModePhase::Complete {
                    RuntimeResetModeOutcomeStateV1::Complete
                } else if mode.phase == ModePhase::Pending {
                    RuntimeResetModeOutcomeStateV1::Pending
                } else {
                    RuntimeResetModeOutcomeStateV1::Incomplete
                },
                resources,
            });
            for resource in self.preserved(target, &paths)? {
                if !preserved.contains(&resource) {
                    preserved.push(resource);
                }
            }
        }
        preserved.sort_by(|left, right| {
            left.path
                .path_bytes_base64url
                .as_bytes()
                .cmp(right.path.path_bytes_base64url.as_bytes())
        });
        Ok(RuntimeResetResultV1 {
            schema: "podway.runtime-reset-result/v1".to_owned(),
            status,
            operation_id: record.operation_id.clone(),
            selection: plan.selection.clone(),
            modes,
            preserved,
            excluded: vec![RuntimeResetExclusionV1 {
                path: None,
                reason: super::document::ExclusionReason::ExternalManagedRuntimes,
            }],
        })
    }

    fn preserved(
        &self,
        target: &super::document::NamespaceBinding,
        paths: &ServiceRuntimePathsV1,
    ) -> Result<Vec<super::document::RuntimeResetResourceV1>, Error> {
        let root = paths
            .podway_home()
            .ok_or_else(Error::unsafe_path)?
            .as_path();
        let mut preserved = vec![super::document::RuntimeResetResourceV1::new(
            RuntimeResetResourceClassV1::NamespaceDirectory,
            root,
        )?];
        if target.run_identity.is_some() {
            preserved.push(super::document::RuntimeResetResourceV1::new(
                RuntimeResetResourceClassV1::NamespaceDirectory,
                &root.join("run"),
            )?);
        }
        if target.lock_identity.is_some() {
            preserved.push(super::document::RuntimeResetResourceV1::new(
                RuntimeResetResourceClassV1::LockAnchor,
                &root.join("run/podwayd.lock"),
            )?);
        }
        if target.registry_lock_identity.is_some() {
            preserved.push(super::document::RuntimeResetResourceV1::new(
                RuntimeResetResourceClassV1::NamespaceDirectory,
                &root.join("state"),
            )?);
            preserved.push(super::document::RuntimeResetResourceV1::new(
                RuntimeResetResourceClassV1::LockAnchor,
                &root.join("state/workspaces.json.lock"),
            )?);
        }
        let maintenance = self.home.as_path().join("maintenance");
        for name in super::document::CONTROL_RESIDUE_NAMES {
            preserved.push(super::document::RuntimeResetResourceV1::new(
                RuntimeResetResourceClassV1::ControlResidue,
                &maintenance.join(name),
            )?);
        }
        preserved.sort_by(|left, right| {
            left.path
                .path_bytes_base64url
                .as_bytes()
                .cmp(right.path.path_bytes_base64url.as_bytes())
        });
        Ok(preserved)
    }
}
