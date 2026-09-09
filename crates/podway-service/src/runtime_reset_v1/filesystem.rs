use std::{
    ffi::{OsStr, OsString},
    fs::File,
    io::Read,
    os::{fd::OwnedFd, unix::ffi::OsStrExt},
    path::{Path, PathBuf},
};

use nix::{
    dir::Dir,
    errno::Errno,
    fcntl::{AtFlags, OFlag, openat},
    sys::stat::{FileStat, Mode, SFlag, fstat, fstatat, mkdirat},
    unistd::fsync,
};
use podway_core::RuntimeModeV1;
use sha2::{Digest, Sha256};

use super::document::{
    FileIdentity, RuntimeResetErrorV1 as Error, RuntimeResetResourceClassV1 as Class,
};
use crate::{PodwayHomeV1, StdServiceFilesystemV1};

pub(super) struct Directory {
    fd: OwnedFd,
    path: PathBuf,
    uid: u32,
    device: u64,
}

#[derive(Clone, Copy)]
pub(super) enum FileKind {
    Regular,
    Socket,
}

fn io_error<T>(_: T) -> Error {
    Error::unsafe_path()
}

fn identity(stat: &FileStat) -> Result<FileIdentity, Error> {
    Ok(FileIdentity {
        device: u64::try_from(stat.st_dev).map_err(io_error)?,
        inode: stat.st_ino,
        uid: stat.st_uid,
        mode: u32::from(stat.st_mode),
        links: u32::from(stat.st_nlink),
    })
}

impl Directory {
    pub(super) fn create_child(&self, name: &OsStr) -> Result<Self, Error> {
        match mkdirat(&self.fd, name, Mode::from_bits_truncate(0o700)) {
            Ok(()) => fsync(&self.fd).map_err(io_error)?,
            Err(Errno::EEXIST) => {}
            Err(error) => return Err(io_error(error)),
        }
        self.child_optional(name, true)?
            .ok_or_else(Error::unsafe_path)
    }

    pub(super) fn open_lock(&self, name: &OsStr) -> Result<File, Error> {
        let before = self.regular_optional(name)?;
        let flags = OFlag::O_RDWR | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC;
        let fd = if before.is_some() {
            openat(&self.fd, name, flags, Mode::empty()).map_err(io_error)?
        } else {
            // Concurrent O_CREAT opens can fail with ENOENT on macOS. Create exactly once;
            // a losing creator opens the existing anchor without allowing its recreation.
            match openat(
                &self.fd,
                name,
                flags | OFlag::O_CREAT | OFlag::O_EXCL,
                Mode::from_bits_truncate(0o600),
            ) {
                Ok(fd) => fd,
                Err(Errno::EEXIST) => {
                    openat(&self.fd, name, flags, Mode::empty()).map_err(io_error)?
                }
                Err(error) => return Err(io_error(error)),
            }
        };
        let after = self.validate_file(&fstat(&fd).map_err(io_error)?, FileKind::Regular)?;
        if before.as_ref().is_some_and(|identity| identity != &after)
            || self.regular_optional(name)?.as_ref() != Some(&after)
        {
            return Err(Error::unsafe_path());
        }
        if before.is_none() {
            fsync(&fd).map_err(io_error)?;
            fsync(&self.fd).map_err(io_error)?;
        }
        Ok(File::from(fd))
    }

    pub(super) fn account(home: &PodwayHomeV1) -> Result<Self, Error> {
        let fd =
            StdServiceFilesystemV1::open_verified_directory_optional(home.account_home(), None)
                .map_err(io_error)?
                .ok_or_else(Error::unsafe_path)?;
        let stat = fstat(&fd).map_err(io_error)?;
        if stat.st_uid != home.user_id() || stat.st_mode & 0o022 != 0 {
            return Err(Error::unsafe_path());
        }
        Ok(Self {
            fd,
            path: home.account_home().to_path_buf(),
            uid: home.user_id(),
            device: identity(&stat)?.device,
        })
    }

    pub(super) fn path(&self) -> &Path {
        &self.path
    }

    pub(super) fn identity(&self) -> Result<FileIdentity, Error> {
        identity(&fstat(&self.fd).map_err(io_error)?)
    }

    pub(super) fn duplicate(&self) -> Result<Self, Error> {
        Ok(Self {
            fd: self.fd.try_clone().map_err(io_error)?,
            path: self.path.clone(),
            uid: self.uid,
            device: self.device,
        })
    }

    pub(super) fn child_optional(
        &self,
        name: &OsStr,
        private: bool,
    ) -> Result<Option<Self>, Error> {
        let fd = match openat(
            &self.fd,
            name,
            OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
            Mode::empty(),
        ) {
            Ok(fd) => fd,
            Err(Errno::ENOENT) => return Ok(None),
            Err(error) => return Err(io_error(error)),
        };
        let stat = fstat(&fd).map_err(io_error)?;
        if stat.st_uid != self.uid
            || identity(&stat)?.device != self.device
            || !identity(&stat)?.same_directory_anchor(&identity(
                &fstatat(&self.fd, name, AtFlags::AT_SYMLINK_NOFOLLOW).map_err(io_error)?,
            )?)
            || (private && stat.st_mode & 0o7777 != 0o700)
            || (!private && stat.st_mode & 0o022 != 0)
        {
            return Err(Error::unsafe_path());
        }
        Ok(Some(Self {
            fd,
            path: self.path.join(name),
            uid: self.uid,
            device: self.device,
        }))
    }

    pub(super) fn entries(&self, bound: usize) -> Result<Vec<OsString>, Error> {
        // A new open file description keeps repeated inventories independent.
        let fd = openat(
            &self.fd,
            ".",
            OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_CLOEXEC,
            Mode::empty(),
        )
        .map_err(io_error)?;
        let mut directory = Dir::from_fd(fd).map_err(io_error)?;
        let mut entries = Vec::new();
        for entry in directory.iter() {
            let entry = entry.map_err(io_error)?;
            let bytes = entry.file_name().to_bytes();
            if bytes == b"." || bytes == b".." {
                continue;
            }
            if entries.len() == bound {
                return Err(Error::limit());
            }
            entries.push(OsStr::from_bytes(bytes).to_os_string());
        }
        entries.sort_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
        Ok(entries)
    }

    pub(super) fn identity_optional(
        &self,
        name: &OsStr,
        kind: FileKind,
    ) -> Result<Option<FileIdentity>, Error> {
        let stat = match fstatat(&self.fd, name, AtFlags::AT_SYMLINK_NOFOLLOW) {
            Ok(stat) => stat,
            Err(Errno::ENOENT) => return Ok(None),
            Err(error) => return Err(io_error(error)),
        };
        self.validate_file(&stat, kind).map(Some)
    }

    fn validate_file(&self, stat: &FileStat, kind: FileKind) -> Result<FileIdentity, Error> {
        let expected = match kind {
            FileKind::Regular => SFlag::S_IFREG,
            FileKind::Socket => SFlag::S_IFSOCK,
        };
        let identity = identity(stat)?;
        if SFlag::from_bits_truncate(stat.st_mode) & SFlag::S_IFMT != expected
            || identity.uid != self.uid
            || identity.device != self.device
            || stat.st_mode & 0o7777 != 0o600
            || identity.links != 1
        {
            return Err(Error::unsafe_path());
        }
        Ok(identity)
    }

    pub(super) fn regular_optional(&self, name: &OsStr) -> Result<Option<FileIdentity>, Error> {
        self.identity_optional(name, FileKind::Regular)
    }

    pub(super) fn open_regular(&self, name: &OsStr) -> Result<File, Error> {
        let before = self
            .regular_optional(name)?
            .ok_or_else(Error::unsafe_path)?;
        let fd = openat(
            &self.fd,
            name,
            OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC,
            Mode::empty(),
        )
        .map_err(io_error)?;
        let after = self.validate_file(&fstat(&fd).map_err(io_error)?, FileKind::Regular)?;
        if before != after {
            return Err(Error::unsafe_path());
        }
        Ok(File::from(fd))
    }

    pub(super) fn read_optional(
        &self,
        name: &OsStr,
        bound: usize,
    ) -> Result<Option<Vec<u8>>, Error> {
        if self.regular_optional(name)?.is_none() {
            return Ok(None);
        }
        let file = self.open_regular(name)?;
        if file.metadata().map_err(io_error)?.len() > bound as u64 {
            return Err(Error::limit());
        }
        let mut bytes = Vec::new();
        file.take(bound as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(io_error)?;
        if bytes.len() > bound {
            return Err(Error::limit());
        }
        Ok(Some(bytes))
    }

    pub(super) fn managed(&self, mode: &RuntimeModeV1) -> Result<bool, Error> {
        let Some(bytes) = self.read_optional(OsStr::new("runtime.json"), 16 * 1024)? else {
            return Ok(false);
        };
        crate::managed_runtime_v3::validate_reset_exclusion_v3(&bytes, &self.path, self.uid, mode)
            .map_err(io_error)?;
        Ok(true)
    }
}

pub(super) fn metadata_digest(
    directory: &Directory,
    name: &OsStr,
    bound: usize,
) -> Result<(FileIdentity, String), Error> {
    let file = directory.open_regular(name)?;
    let before = fstat(&file).map_err(io_error)?;
    if before.st_size < 0 || before.st_size as u64 > bound as u64 {
        return Err(Error::limit());
    }
    let mut bytes = Vec::new();
    (&file)
        .take(bound as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(io_error)?;
    let after = fstat(&file).map_err(io_error)?;
    let identity = identity(&before)?;
    if bytes.len() > bound {
        return Err(Error::limit());
    }
    if directory.regular_optional(name)?.as_ref() != Some(&identity)
        || before.st_size != after.st_size
        || before.st_mtime != after.st_mtime
        || before.st_mtime_nsec != after.st_mtime_nsec
    {
        return Err(Error::unsafe_path());
    }
    Ok((identity, format!("sha256:{:x}", Sha256::digest(bytes))))
}

pub(super) fn state_resource_class(name: &OsStr) -> Option<Class> {
    let name = name.to_str()?;
    match name {
        "workspaces.json" => Some(Class::Registry),
        "recovery.json" => Some(Class::RuntimeRecovery),
        "service.json" => Some(Class::ServiceMetadata),
        _ if StdServiceFilesystemV1::is_owned_temporary_name_v1(
            OsStr::new("service.json"),
            name,
        ) =>
        {
            Some(Class::ServiceMetadata)
        }
        _ => {
            let remainder = name
                .strip_prefix(".podway-registry-v1-")?
                .strip_suffix(".tmp")?;
            let (pid, sequence) = remainder.split_once('-')?;
            let parsed_pid = pid.parse::<u32>().ok()?;
            let parsed_sequence = sequence.parse::<u64>().ok()?;
            (pid == parsed_pid.to_string() && sequence == parsed_sequence.to_string())
                .then_some(Class::Registry)
        }
    }
}

pub(super) fn log_resource_class(name: &OsStr) -> Option<Class> {
    let name = name.to_str()?;
    for (base, class, retained) in [
        (
            "podwayd.log",
            Class::DaemonLog,
            usize::from(crate::SERVICE_LOG_RETAINED_FILES_V1),
        ),
        (
            "podwayd-bootstrap.log",
            Class::BootstrapLog,
            usize::from(crate::SERVICE_BOOTSTRAP_LOG_RETAINED_FILES_V1),
        ),
    ] {
        if name == base {
            return Some(class);
        }
        if let Some(suffix) = name
            .strip_prefix(base)
            .and_then(|value| value.strip_prefix('.'))
            && let Ok(index) = suffix.parse::<usize>()
            && (1..retained).contains(&index)
            && suffix == index.to_string()
        {
            return Some(class);
        }
    }
    None
}
