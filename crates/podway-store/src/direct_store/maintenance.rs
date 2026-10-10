use std::{
    fs::TryLockError,
    fs::{self, File, OpenOptions},
    io,
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::Path,
    thread,
    time::{Duration, Instant},
};

use nix::{
    errno::Errno,
    fcntl::{OFlag, openat},
    sys::stat::{Mode, mkdirat},
};

use super::classification::{classify_direct_store_with_missing_observer_v1, maintenance_marked};
use super::{
    DirectStoreClassificationV1, DirectStoreErrorV1, DirectStoreResultV1, DirectWorkspaceLayoutV1,
};

// Empty-residual directory admission in Engine workspace.rs uses the same cadence.
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
                match ensure_runtime_directories(layout) {
                    Ok(()) => {}
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {
                        // Final removal can retire this namespace while it is
                        // being opened. Retry the native creation boundary,
                        // without extending the original wait deadline.
                        wait(deadline)?;
                        continue;
                    }
                    Err(error) => return Err(error.into()),
                }
            }
            let podway = match open_directory(&layout.podway_directory()) {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    if create {
                        wait(deadline)?;
                        continue;
                    }
                    return Err(DirectStoreErrorV1::NotInitialized);
                }
                Err(error) => return Err(error.into()),
            };
            let directory = match open_directory(&layout.runtime_directory()) {
                Ok(file) => file,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    if create {
                        wait(deadline)?;
                        continue;
                    }
                    return Err(DirectStoreErrorV1::NotInitialized);
                }
                Err(error) => return Err(error.into()),
            };
            // The directory inode remains the exclusion anchor even after final
            // removal unlinks the named lock files. Do not recreate those files
            // until this exact runtime directory is exclusively held.
            while !try_lock(&directory, true)? {
                wait(deadline)?;
            }
            if !same_file(&podway, &layout.podway_directory())?
                || !same_file(&directory, &layout.runtime_directory())?
            {
                wait(deadline)?;
                continue;
            }
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
    let podway = ensure_child_directory(&root, &layout.root().to_path_buf(), ".podway")?;
    ensure_child_directory(&podway, &layout.podway_directory(), "runtime")?;
    root.sync_all()
}

fn ensure_child_directory(parent: &File, parent_path: &Path, name: &str) -> io::Result<File> {
    // A final remover can retire the parent between path lookup and mkdir. Use
    // its held descriptor so creation cannot resolve through a replacement.
    match mkdirat(parent, name, Mode::from_bits_truncate(0o700)) {
        Ok(()) | Err(Errno::EEXIST) => {}
        Err(Errno::EINVAL) if !same_file(parent, parent_path)? => {
            return Err(io::ErrorKind::NotFound.into());
        }
        Err(error) => return Err(error.into()),
    }
    let directory = File::from(openat(
        parent,
        name,
        OFlag::O_CLOEXEC | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_RDONLY,
        Mode::empty(),
    )?);
    directory.sync_all()?;
    Ok(directory)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn retired_parent_creation_does_not_publish_into_its_replacement() {
        let temporary = tempfile::TempDir::new().unwrap();
        let path = temporary.path().canonicalize().unwrap().join(".podway");
        fs::create_dir(&path).unwrap();
        let retired = open_directory(&path).unwrap();
        fs::remove_dir(&path).unwrap();
        fs::create_dir(&path).unwrap();
        let error = ensure_child_directory(&retired, &path, "runtime").unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
        assert_eq!(fs::read_dir(&path).unwrap().count(), 0);
        let current = open_directory(&path).unwrap();
        let runtime = ensure_child_directory(&current, &path, "runtime").unwrap();
        assert!(same_file(&runtime, &path.join("runtime")).unwrap());
        assert_eq!(fs::read_dir(path.join("runtime")).unwrap().count(), 0);
    }
}
