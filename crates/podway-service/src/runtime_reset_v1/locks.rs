use std::{ffi::OsStr, fs::File, thread, time::Instant};

use nix::{
    errno::Errno,
    fcntl::{Flock, FlockArg},
};
use podway_core::RuntimeModeV1;

use super::{
    MAX_RUNTIME_RESET_DOCUMENT_BYTES_V1, RuntimeResetErrorV1, RuntimeResetOperationV1,
    RuntimeResetPathV1, RuntimeResetProcessV1,
    document::{FileIdentity, ResetRecord},
    filesystem::Directory,
};
use crate::{PodwayHomeV1, SERVICE_LIFECYCLE_LOCK_RETRY_V1, SERVICE_LIFECYCLE_LOCK_TIMEOUT_V1};

/// One retained account lock anchor. Dropping the guard unlocks without unlinking it.
pub struct RuntimeResetLockV1 {
    _file: Flock<File>,
}

impl RuntimeResetLockV1 {
    /// Holds the topology gate through ordinary namespace preparation and endpoint binding.
    pub fn prepare_start(
        home: &PodwayHomeV1,
        mode: &RuntimeModeV1,
    ) -> Result<Self, RuntimeResetErrorV1> {
        let guard = Self::gate_start(home, mode)?;
        let account = Directory::account(home)?;
        let root = account.create_child(OsStr::new(".podway"))?;
        let namespace = if mode.is_production() {
            root
        } else {
            root.create_child(OsStr::new("modes"))?
                .create_child(OsStr::new(mode.as_str()))?
        };
        namespace.create_child(OsStr::new("run"))?;
        Ok(guard)
    }

    /// Checks the committed fence while retaining the topology gate, without creating a namespace.
    pub fn gate_start(
        home: &PodwayHomeV1,
        mode: &RuntimeModeV1,
    ) -> Result<Self, RuntimeResetErrorV1> {
        let guard = Self::acquire_topology(home)?;
        RuntimeResetRecordViewV1::ensure_start_allowed(home, mode)?;
        Ok(guard)
    }

    /// Competing reset operations refuse immediately instead of waiting for destructive work.
    pub fn acquire_reset(home: &PodwayHomeV1) -> Result<Self, RuntimeResetErrorV1> {
        Self::acquire(home, "runtime-reset.lock", false)
    }

    pub fn acquire_topology(home: &PodwayHomeV1) -> Result<Self, RuntimeResetErrorV1> {
        Self::acquire(home, "runtime-start.lock", true)
    }

    /// Reservation release takes this lock before the daemon admission mutex.
    pub fn acquire_commit(home: &PodwayHomeV1) -> Result<Self, RuntimeResetErrorV1> {
        Self::acquire(home, "runtime-reset-commit.lock", true)
    }

    fn acquire(home: &PodwayHomeV1, name: &str, wait: bool) -> Result<Self, RuntimeResetErrorV1> {
        let account = Directory::account(home)?;
        let root = account.create_child(OsStr::new(".podway"))?;
        let maintenance = root.create_child(OsStr::new("maintenance"))?;
        let mut file = maintenance.open_lock(OsStr::new(name))?;
        let anchor = maintenance.regular_optional(OsStr::new(name))?;
        let deadline = Instant::now() + SERVICE_LIFECYCLE_LOCK_TIMEOUT_V1;
        loop {
            match Flock::lock(file, FlockArg::LockExclusiveNonblock) {
                Ok(file) => {
                    let root_identity = root.identity()?;
                    let maintenance_identity = maintenance.identity()?;
                    if maintenance.regular_optional(OsStr::new(name))? != anchor
                        || !account
                            .child_optional(OsStr::new(".podway"), true)?
                            .map(|current| current.identity())
                            .transpose()?
                            .is_some_and(|current| current.same_directory_anchor(&root_identity))
                        || !root
                            .child_optional(OsStr::new("maintenance"), true)?
                            .map(|current| current.identity())
                            .transpose()?
                            .is_some_and(|current| {
                                current.same_directory_anchor(&maintenance_identity)
                            })
                    {
                        return Err(RuntimeResetErrorV1::unsafe_path());
                    }
                    return Ok(Self { _file: file });
                }
                Err((returned, Errno::EWOULDBLOCK)) if wait && Instant::now() < deadline => {
                    file = returned;
                    thread::sleep(SERVICE_LIFECYCLE_LOCK_RETRY_V1);
                }
                Err((_, Errno::EWOULDBLOCK)) => return Err(RuntimeResetErrorV1::in_progress()),
                Err(_) => return Err(RuntimeResetErrorV1::unsafe_path()),
            }
        }
    }
}

/// The validated participant identity needed by admission and startup fencing.
#[derive(Clone, Debug)]
pub struct RuntimeResetParticipantV1 {
    pub mode: RuntimeModeV1,
    pub root: RuntimeResetPathV1,
    pub process: Option<RuntimeResetProcessV1>,
    pub reservation_id: Option<String>,
    pub stop_intended: bool,
}

/// A read-only projection of an in-progress record, never a lock acquisition.
#[derive(Clone, Debug)]
pub struct RuntimeResetRecordViewV1 {
    pub operation: RuntimeResetOperationV1,
    pub participants: Vec<RuntimeResetParticipantV1>,
    pub(super) account_identity: FileIdentity,
}

impl RuntimeResetRecordViewV1 {
    /// Callers own their required topology or commit interlock. In particular, remote
    /// reservation snapshots must be readable while their caller holds the commit interlock.
    pub fn read(home: &PodwayHomeV1) -> Result<Option<Self>, RuntimeResetErrorV1> {
        let account = Directory::account(home)?;
        let Some(root) = account.child_optional(OsStr::new(".podway"), true)? else {
            return Ok(None);
        };
        let Some(maintenance) = root.child_optional(OsStr::new("maintenance"), true)? else {
            return Ok(None);
        };
        let Some(bytes) = maintenance.read_optional(
            OsStr::new("runtime-reset.json"),
            MAX_RUNTIME_RESET_DOCUMENT_BYTES_V1,
        )?
        else {
            return Ok(None);
        };
        let view = ResetRecord::view(&bytes, home)?;
        if let Some(view) = &view {
            let actual = root.identity()?;
            let expected = &view.account_identity;
            // Retirement can remove owned child directories and change the directory's link
            // count. The account anchor itself must retain its inode, owner and private mode.
            if !actual.same_directory_anchor(expected) {
                return Err(RuntimeResetErrorV1::unsafe_path());
            }
        }
        Ok(view)
    }

    pub fn participant(&self, mode: &RuntimeModeV1) -> Option<&RuntimeResetParticipantV1> {
        self.participants
            .iter()
            .find(|participant| participant.mode == *mode)
    }

    pub fn ensure_start_allowed(
        home: &PodwayHomeV1,
        mode: &RuntimeModeV1,
    ) -> Result<(), RuntimeResetErrorV1> {
        if Self::read(home)?.is_some_and(|record| record.participant(mode).is_some()) {
            return Err(RuntimeResetErrorV1::in_progress().in_mode(mode));
        }
        Ok(())
    }
}
