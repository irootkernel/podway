use std::{
    fs::TryLockError,
    fs::{self, File, OpenOptions},
    io,
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
    path::Path,
    thread,
    time::{Duration, Instant},
};

use nix::fcntl::OFlag;

use super::classification::{classify_direct_store_with_missing_observer_v1, maintenance_marked};
use super::{
    DirectStoreClassificationV1, DirectStoreErrorV1, DirectStoreResultV1, DirectWorkspaceLayoutV1,
};

const LOCK_RETRY_SLEEP: Duration = Duration::from_micros(500);

/// Ordinary opens start shared. Missing WAL or SHM requires stronger exclusion:
/// a stable immutable main file or a write-free heap WAL snapshot. All direct
/// writers obey this native lease; unix-flock and POSIX locks are distinct.
pub(super) enum DirectOpenLease {
    Shared(SharedMaintenanceLeaseV1),
    Cold(ExclusiveMaintenanceLeaseV1),
}

impl DirectOpenLease {
    pub(super) fn acquire(
        layout: &DirectWorkspaceLayoutV1,
        deadline: Instant,
        missing: &mut dyn FnMut(),
    ) -> DirectStoreResultV1<Self> {
        let shared =
            SharedMaintenanceLeaseV1::acquire_with_missing_observer(layout, deadline, missing)?;
        if super::classification::path_exists(&layout.wal())?
            && super::classification::path_exists(&layout.shm())?
        {
            return Ok(Self::Shared(shared));
        }
        drop(shared);
        let exclusive = match ExclusiveMaintenanceLeaseV1::acquire(layout, deadline, false) {
            Ok(lease) => lease,
            Err(DirectStoreErrorV1::NotInitialized) => {
                classify_direct_store_with_missing_observer_v1(layout, missing)?
                    .require_current()?;
                return Err(DirectStoreErrorV1::Maintenance);
            }
            Err(error) => return Err(error),
        };
        classify_direct_store_with_missing_observer_v1(layout, missing)?.require_current()?;
        Ok(Self::Cold(exclusive))
    }

    pub(super) fn heap_snapshot(&self) -> bool {
        matches!(self, Self::Cold(_))
    }

    pub(super) fn revalidate(&self, layout: &DirectWorkspaceLayoutV1) -> DirectStoreResultV1<()> {
        match self {
            Self::Shared(lease) => lease.revalidate(layout),
            Self::Cold(lease) => {
                if maintenance_marked(layout)? {
                    return Err(DirectStoreErrorV1::Maintenance);
                }
                lease.revalidate(layout)
            }
        }
    }
}

/// A real native shared lease, held throughout an ordinary Store open and snapshot.
#[derive(Debug)]
pub struct SharedMaintenanceLeaseV1 {
    podway: File,
    directory: File,
    lock: File,
}

impl SharedMaintenanceLeaseV1 {
    /// Does not create directories, lock files, marker files or SQLite sidecars.
    pub fn acquire(
        layout: &DirectWorkspaceLayoutV1,
        deadline: Instant,
    ) -> DirectStoreResultV1<Self> {
        Self::acquire_with_missing_observer(layout, deadline, &mut || {})
    }

    fn acquire_with_missing_observer(
        layout: &DirectWorkspaceLayoutV1,
        deadline: Instant,
        missing: &mut dyn FnMut(),
    ) -> DirectStoreResultV1<Self> {
        classify_direct_store_with_missing_observer_v1(layout, missing)?.require_current()?;
        loop {
            if maintenance_marked(layout)? {
                return Err(DirectStoreErrorV1::Maintenance);
            }
            let podway = match open_directory(&layout.podway_directory()) {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    retry_missing(layout, deadline, missing)?;
                    continue;
                }
                Err(error) => return Err(error.into()),
            };
            let directory = match open_directory(&layout.runtime_directory()) {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    retry_missing(layout, deadline, missing)?;
                    continue;
                }
                Err(error) => return Err(error.into()),
            };
            let intent = match open_lock(&layout.intent_lock(), false) {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    retry_missing(layout, deadline, missing)?;
                    continue;
                }
                Err(error) => return Err(error.into()),
            };
            if !try_lock(&intent, false)? {
                wait(deadline)?;
                continue;
            }
            if !same_file(&intent, &layout.intent_lock())? {
                wait(deadline)?;
                continue;
            }
            let lock = match open_lock(&layout.maintenance_lock(), false) {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    retry_missing(layout, deadline, missing)?;
                    continue;
                }
                Err(error) => return Err(error.into()),
            };
            if !try_lock(&lock, false)? {
                wait(deadline)?;
                continue;
            }
            if !same_file(&podway, &layout.podway_directory())?
                || !same_file(&directory, &layout.runtime_directory())?
                || !same_file(&intent, &layout.intent_lock())?
                || !same_file(&lock, &layout.maintenance_lock())?
            {
                wait(deadline)?;
                continue;
            }
            // Classification before waiting is not authority for the replacement generation.
            classify_direct_store_with_missing_observer_v1(layout, missing)?.require_current()?;
            drop(intent);
            return Ok(Self {
                podway,
                directory,
                lock,
            });
        }
    }

    pub(super) fn revalidate(&self, layout: &DirectWorkspaceLayoutV1) -> DirectStoreResultV1<()> {
        if maintenance_marked(layout)? {
            return Err(DirectStoreErrorV1::Maintenance);
        }
        if !same_file(&self.podway, &layout.podway_directory())?
            || !same_file(&self.directory, &layout.runtime_directory())?
            || !same_file(&self.lock, &layout.maintenance_lock())?
        {
            return Err(DirectStoreErrorV1::Maintenance);
        }
        Ok(())
    }
}

/// Holds intent exclusively before waiting for existing readers and writers to drain.
/// The engine owns the typed persistent reset/removal marker and its recovery protocol.
#[derive(Debug)]
pub struct ExclusiveMaintenanceLeaseV1 {
    podway: File,
    directory: File,
    intent: File,
    lock: File,
}

impl ExclusiveMaintenanceLeaseV1 {
    /// Only maintenance may create the native runtime directories and lock files.
    pub fn acquire(
        layout: &DirectWorkspaceLayoutV1,
        deadline: Instant,
        create: bool,
    ) -> DirectStoreResultV1<Self> {
        loop {
            if create {
                ensure_runtime_directories(layout)?;
            }
            let podway = match open_directory(&layout.podway_directory()) {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    return Err(DirectStoreErrorV1::NotInitialized);
                }
                Err(error) => return Err(error.into()),
            };
            let directory = match open_directory(&layout.runtime_directory()) {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    return Err(DirectStoreErrorV1::NotInitialized);
                }
                Err(error) => return Err(error.into()),
            };
            let intent = match open_lock(&layout.intent_lock(), create) {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    wait(deadline)?;
                    continue;
                }
                Err(error) => return Err(error.into()),
            };
            while !try_lock(&intent, true)? {
                wait(deadline)?;
            }
            if !same_file(&intent, &layout.intent_lock())? {
                wait(deadline)?;
                continue;
            }
            let lock = match open_lock(&layout.maintenance_lock(), create) {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    wait(deadline)?;
                    continue;
                }
                Err(error) => return Err(error.into()),
            };
            while !try_lock(&lock, true)? {
                wait(deadline)?;
            }
            if !same_file(&podway, &layout.podway_directory())?
                || !same_file(&directory, &layout.runtime_directory())?
                || !same_file(&intent, &layout.intent_lock())?
                || !same_file(&lock, &layout.maintenance_lock())?
            {
                wait(deadline)?;
                continue;
            }
            return Ok(Self {
                podway,
                directory,
                intent,
                lock,
            });
        }
    }

    /// Bind a maintenance primitive to this lease's current path and inodes.
    pub fn revalidate(&self, layout: &DirectWorkspaceLayoutV1) -> DirectStoreResultV1<()> {
        if !same_file(&self.podway, &layout.podway_directory())?
            || !same_file(&self.directory, &layout.runtime_directory())?
            || !same_file(&self.intent, &layout.intent_lock())?
            || !same_file(&self.lock, &layout.maintenance_lock())?
        {
            return Err(DirectStoreErrorV1::Maintenance);
        }
        Ok(())
    }
}

fn ensure_runtime_directories(layout: &DirectWorkspaceLayoutV1) -> io::Result<()> {
    // A resolver-owned root must already exist; maintenance never creates a worktree.
    let root = open_directory(&layout.root().to_path_buf())?;
    for path in [layout.podway_directory(), layout.runtime_directory()] {
        match fs::DirBuilder::new().mode(0o700).create(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
        let directory = open_directory(&path)?;
        directory.sync_all()?;
    }
    root.sync_all()
}

pub(super) fn open_directory(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .custom_flags((OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW).bits())
        .open(path)
}

fn open_lock(path: &Path, create: bool) -> io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .write(create)
        .create(create)
        .mode(0o600)
        .custom_flags(OFlag::O_NOFOLLOW.bits())
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::other("maintenance lock must be a regular file"));
    }
    Ok(file)
}

fn try_lock(file: &File, exclusive: bool) -> io::Result<bool> {
    let result = if exclusive {
        file.try_lock()
    } else {
        file.try_lock_shared()
    };
    match result {
        Ok(()) => Ok(true),
        Err(TryLockError::WouldBlock) => Ok(false),
        Err(TryLockError::Error(error)) => Err(error),
    }
}

pub(super) fn same_file(file: &File, path: &Path) -> io::Result<bool> {
    let held = file.metadata()?;
    match fs::symlink_metadata(path) {
        Ok(current) => Ok(!current.file_type().is_symlink()
            && held.dev() == current.dev()
            && held.ino() == current.ino()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

fn retry_missing(
    layout: &DirectWorkspaceLayoutV1,
    deadline: Instant,
    missing: &mut dyn FnMut(),
) -> DirectStoreResultV1<()> {
    // A reset may already have completed and removed its marker. Reclassify the actual
    // replacement rather than returning a stale missing-lock result from the old generation.
    match classify_direct_store_with_missing_observer_v1(layout, missing)? {
        DirectStoreClassificationV1::Current => wait(deadline),
        classification => classification.require_current(),
    }
}

fn check_deadline(deadline: Instant) -> DirectStoreResultV1<()> {
    if Instant::now() >= deadline {
        return Err(DirectStoreErrorV1::Maintenance);
    }
    Ok(())
}

fn wait(deadline: Instant) -> DirectStoreResultV1<()> {
    check_deadline(deadline)?;
    thread::sleep(LOCK_RETRY_SLEEP.min(deadline.saturating_duration_since(Instant::now())));
    check_deadline(deadline)
}
