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

/// Applies one explicitly selected ordinary runtime and resumes its exact durable operation.
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
        let RuntimeResetSelectionV1::Mode { .. } = &selection else {
            return Err(Error::unsupported_reason(
                RuntimeResetReasonV1::UnsupportedPeer,
            ));
        };
        let token_sha256 = format!("sha256:{:x}", Sha256::digest(encoded_token.as_bytes()));
        let _reset = RuntimeResetLockV1::acquire_reset(&self.home)?;
        let selected_mode = match &selection {
            RuntimeResetSelectionV1::Mode { mode } => mode,
            RuntimeResetSelectionV1::AllModes => unreachable!("single-mode checked above"),
        };
        self.operator.prepare(&self.paths(selected_mode)?)?;
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
        let token = ResetToken::decode(encoded_token)?;
        if token.targets.len() != 1 {
            return Err(Error::unsupported_reason(
                RuntimeResetReasonV1::UnsupportedPeer,
            ));
        }
        let operation = RuntimeResetOperationV1 {
            operation_id: uuid::Uuid::new_v4().to_string(),
            token_sha256,
        };
        let target = token.targets[0].clone();
        let paths = self.paths(&target.mode)?;
        let reservation = if let Some(process) = &target.process {
            Some(self.operator.reserve(&paths, &operation, process)?)
        } else {
            None
        };

        let commit = RuntimeResetLockV1::acquire_commit(&self.home)?;
        let commit_result = (|| {
            if let (Some(process), Some(reservation)) = (&target.process, &reservation) {
                self.operator
                    .snapshot(&paths, &operation, process, reservation)?;
            }
            let record = ResetRecord {
                schema: "podway.runtime-reset-record/v1".to_owned(),
                operation_id: operation.operation_id.clone(),
                token_sha256: operation.token_sha256.clone(),
                phase: RecordPhase::InProgress,
                plan: Some(token),
                modes: vec![RecordMode {
                    mode: target.mode.clone(),
                    phase: ModePhase::Pending,
                    reservation_id: reservation.clone(),
                    entries: Vec::new(),
                }],
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
                if let (Some(process), Some(reservation)) = (&target.process, &reservation) {
                    self.operator
                        .release_uncommitted(&paths, &operation, process, reservation);
                }
                return Err(error);
            }
        };
        match self.resume(&root, &maintenance, record, false) {
            Ok(result) => Ok(result),
            Err(error) => Err(self.committed_error(error, &maintenance, None)),
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
        let target = record.plan.as_ref().expect("validated record").targets[0].clone();
        let paths = self.paths(&target.mode)?;
        if reconnect
            && matches!(
                record.modes[0].phase,
                ModePhase::Pending | ModePhase::StopIntended
            )
            && let Some(process) = &target.process
        {
            let reservation = self.operator.reserve(&paths, &operation, process)?;
            let expected = record.modes[0]
                .reservation_id
                .as_deref()
                .ok_or_else(|| Error::unsafe_reason(RuntimeResetReasonV1::ReservationLost))?;
            if reservation != expected {
                return Err(Error::unsafe_reason(RuntimeResetReasonV1::ReservationLost));
            }
        }

        if record.modes[0].phase == ModePhase::Pending {
            record.modes[0].phase = ModePhase::StopIntended;
            self.persist(maintenance, &record)?;
            self.operator.checkpoint("stop_intended")?;
        }
        if record.modes[0].phase == ModePhase::StopIntended {
            self.operator.stop(
                &paths,
                &operation,
                target.process.as_ref(),
                record.modes[0].reservation_id.as_deref(),
            )?;
            self.operator
                .wait_stopped(&paths, target.process.as_ref())?;
            self.verify_singleton(root, &target)?;
            record.modes[0].phase = ModePhase::Stopped;
            self.persist(maintenance, &record)?;
            self.operator.checkpoint("stopped")?;
        }
        if record.modes[0].phase == ModePhase::Stopped {
            record.modes[0].phase = ModePhase::InventoryIntended;
            self.persist(maintenance, &record)?;
            self.operator.checkpoint("inventory_intended")?;
        }
        if record.modes[0].phase == ModePhase::InventoryIntended {
            record.modes[0].entries = self.inventory(root, &target)?;
            record.modes[0].phase = ModePhase::InventoryReady;
            self.persist(maintenance, &record)?;
            self.operator.checkpoint("inventory_ready")?;
        }

        for index in 0..record.modes[0].entries.len() {
            if record.modes[0].entries[index].state == EntryState::Completed {
                continue;
            }
            let had_intent = record.modes[0].entries[index].state == EntryState::Intended;
            if !had_intent {
                record.modes[0].entries[index].state = EntryState::Intended;
                self.persist(maintenance, &record)?;
                self.operator.checkpoint("unlink_intended")?;
            }
            let removed = self.remove_entry(root, &target, &record.modes[0].entries[index])?;
            if !removed && !had_intent {
                return Err(Error::unsafe_reason(RuntimeResetReasonV1::ResourceChanged));
            }
            self.operator.checkpoint("unlinked")?;
            record.modes[0].entries[index].state = EntryState::Completed;
            self.persist(maintenance, &record)?;
            self.operator.checkpoint("unlink_completed")?;
        }
        record.modes[0].phase = ModePhase::Complete;
        self.persist(maintenance, &record)?;

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
                if let Some(mode) = result.modes.first_mut() {
                    mode.state = RuntimeResetModeOutcomeStateV1::Incomplete;
                }
                return Error::incomplete(error.reason, result);
            }
            return error;
        }
        match self.result_with_status(record, RuntimeResetResultStatusV1::Incomplete) {
            Ok(result) => Error::incomplete(error.reason, result),
            Err(_) => error,
        }
    }

    fn result_with_status(
        &self,
        record: &ResetRecord,
        status: RuntimeResetResultStatusV1,
    ) -> Result<RuntimeResetResultV1, Error> {
        let plan = record.plan.as_ref().expect("in-progress record");
        let target = &plan.targets[0];
        let mut resources = record.modes[0]
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
            record.modes[0].phase,
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
        Ok(RuntimeResetResultV1 {
            schema: "podway.runtime-reset-result/v1".to_owned(),
            status,
            operation_id: record.operation_id.clone(),
            selection: plan.selection.clone(),
            modes: vec![RuntimeResetModeOutcomeV1 {
                mode: target.mode.clone(),
                state: if status == RuntimeResetResultStatusV1::Incomplete {
                    RuntimeResetModeOutcomeStateV1::Incomplete
                } else if record.modes[0].phase == ModePhase::Complete {
                    RuntimeResetModeOutcomeStateV1::Complete
                } else if record.modes[0].phase == ModePhase::Pending {
                    RuntimeResetModeOutcomeStateV1::Pending
                } else {
                    RuntimeResetModeOutcomeStateV1::Incomplete
                },
                resources,
            }],
            preserved: self.preserved(target, &paths)?,
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
        for name in [
            "runtime-reset.lock",
            "runtime-start.lock",
            "runtime-reset-commit.lock",
            "runtime-reset.json",
        ] {
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
