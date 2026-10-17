//! Descriptor-relative Procedure reads and atomic replacement.

use std::{
    ffi::{OsStr, OsString},
    fs::{self},
    io::{Read, Write},
    os::{fd::OwnedFd, unix::fs::MetadataExt},
    path::{Component, Path},
};

use nix::{
    errno::Errno,
    fcntl::{OFlag, open, openat, renameat},
    sys::stat::{Mode, fchmod, mode_t},
    unistd::{UnlinkatFlags, fsync, unlinkat},
};
use podway_config::{MAX_PROCEDURE_DOCUMENT_BYTES, ProcedureDocumentFormat};

use super::{LocalFailure, PROCEDURE_FORMAT_COMMAND, WorkspaceTarget};

pub(super) fn procedure_format(path: &Path) -> ProcedureDocumentFormat {
    if path.extension().and_then(OsStr::to_str) == Some("json") {
        ProcedureDocumentFormat::Json
    } else {
        ProcedureDocumentFormat::Yaml
    }
}

pub(super) fn read_worktree_procedure(
    root: &WorkspaceTarget,
    procedure: &Path,
) -> Result<Vec<u8>, LocalFailure> {
    read_descriptor_relative_procedure(
        &root.path,
        procedure,
        LocalFailure::catalog("PATH_OUTSIDE_WORKTREE", "cannot open worktree", "procedure"),
    )
}

pub(super) fn read_offline_procedure(procedure: &Path) -> Result<Vec<u8>, LocalFailure> {
    open_offline_procedure(procedure).map(|opened| opened.bytes)
}

/// Opens a procedure named by an ordinary filesystem path, keeping everything a rewrite needs.
///
/// The path is split into a canonicalized parent and a leaf so the hardened descriptor walk applies
/// to a command-line path exactly as it applies to a worktree-relative one.
pub(super) fn open_offline_procedure(procedure: &Path) -> Result<OpenedProcedure, LocalFailure> {
    let parent = procedure
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let file_name = procedure
        .file_name()
        .ok_or_else(|| LocalFailure::procedure_not_found("procedure file is not specified"))?;
    let root = fs::canonicalize(parent)
        .map_err(|_| LocalFailure::procedure_not_found("cannot read procedure file"))?;
    open_descriptor_relative_procedure(
        &root,
        Path::new(file_name),
        LocalFailure::procedure_not_found("cannot read procedure file"),
    )
}

/// A procedure file the hardened walk has opened and read, left addressable for a rewrite.
///
/// Holding the parent descriptor is what lets `format --write` replace the file without ever
/// re-resolving a name: every later operation is `*at`-relative to this one directory, so no
/// component of the path can be swapped between the read and the rename.
pub(super) struct OpenedProcedure {
    /// The directory the leaf lives in, reached by opening every component `O_NOFOLLOW`.
    parent: OwnedFd,
    /// The final path component, exactly as the request spelled it.
    leaf: OsString,
    /// The document bytes.
    bytes: Vec<u8>,
    /// The leaf's permission bits, taken from the descriptor the bytes were read through — the mode
    /// therefore belongs to the content, with no second lookup for anything to race.
    mode: Mode,
}

impl OpenedProcedure {
    pub(super) fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

fn read_descriptor_relative_procedure(
    root: &Path,
    procedure: &Path,
    root_failure: LocalFailure,
) -> Result<Vec<u8>, LocalFailure> {
    open_descriptor_relative_procedure(root, procedure, root_failure).map(|opened| opened.bytes)
}

/// Opens the directory holding a descriptor-relative procedure path and returns it with the leaf
/// name, so reading and rewriting share one component walk rather than two that can drift.
///
/// Every component is opened `O_CLOEXEC|O_DIRECTORY|O_NOFOLLOW|O_RDONLY`, only
/// [`Component::Normal`] components are accepted — `..`, a root, and a prefix are all
/// `PATH_OUTSIDE_WORKTREE` — and every open failure maps through [`procedure_open_failure`], which
/// turns the `ELOOP` of a symlinked component into the same path rejection.
fn open_descriptor_relative_parent(
    root: &Path,
    procedure: &Path,
    root_failure: LocalFailure,
) -> Result<(OwnedFd, OsString), LocalFailure> {
    let mut directory = open(
        root,
        OFlag::O_CLOEXEC | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_RDONLY,
        Mode::empty(),
    )
    .map_err(|_| root_failure)?;
    let mut components = procedure.components().peekable();
    while let Some(component) = components.next() {
        let Component::Normal(component) = component else {
            return Err(LocalFailure::catalog(
                "PATH_OUTSIDE_WORKTREE",
                "procedure must be worktree-relative",
                "procedure",
            ));
        };
        if components.peek().is_none() {
            return Ok((directory, component.to_owned()));
        }
        directory = openat(
            &directory,
            component,
            OFlag::O_CLOEXEC | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_RDONLY,
            Mode::empty(),
        )
        .map_err(procedure_open_failure)?;
    }
    Err(LocalFailure::procedure_not_found(
        "procedure file is not specified",
    ))
}

/// Opens, size-bounds, and reads a descriptor-relative procedure file.
///
/// The leaf is opened `O_NOFOLLOW`, so a symlink named as the procedure is refused rather than
/// followed, and the descriptor the bytes come from is the descriptor its permission bits come
/// from.
pub(super) fn open_descriptor_relative_procedure(
    root: &Path,
    procedure: &Path,
    root_failure: LocalFailure,
) -> Result<OpenedProcedure, LocalFailure> {
    let (parent, leaf) = open_descriptor_relative_parent(root, procedure, root_failure)?;
    let descriptor = openat(
        &parent,
        leaf.as_os_str(),
        OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_RDONLY,
        Mode::empty(),
    )
    .map_err(procedure_open_failure)?;
    let file = fs::File::from(descriptor);
    let metadata = file
        .metadata()
        .map_err(|_| LocalFailure::procedure_not_found("cannot read procedure file"))?;
    if !metadata.file_type().is_file() {
        return Err(LocalFailure::procedure_invalid(
            "procedure must be a regular file",
        ));
    }
    if metadata.len() > MAX_PROCEDURE_DOCUMENT_BYTES as u64 {
        return Err(LocalFailure::procedure_invalid(
            "procedure exceeds the maximum document size",
        ));
    }
    let mode = permission_bits(&metadata);
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(MAX_PROCEDURE_DOCUMENT_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| LocalFailure::procedure_not_found("cannot read procedure file"))?;
    if bytes.len() > MAX_PROCEDURE_DOCUMENT_BYTES {
        return Err(LocalFailure::procedure_invalid(
            "procedure exceeds the maximum document size",
        ));
    }
    Ok(OpenedProcedure {
        parent,
        leaf,
        bytes,
        mode,
    })
}

/// The twelve permission bits of a file, as the mode argument the `*at` calls take.
///
/// `mode_t` is 16 bits on some targets and 32 on others; masking first means the value always fits,
/// so the fallback is unreachable and only exists to keep the conversion total.
fn permission_bits(metadata: &fs::Metadata) -> Mode {
    Mode::from_bits_truncate(mode_t::try_from(metadata.mode() & 0o7777).unwrap_or(0o600))
}

fn procedure_open_failure(error: Errno) -> LocalFailure {
    if error == Errno::ELOOP {
        LocalFailure::catalog(
            "PATH_OUTSIDE_WORKTREE",
            "procedure source must not traverse symlinks",
            "procedure",
        )
    } else {
        LocalFailure::procedure_not_found("cannot read procedure file")
    }
}

/// How many numbered temporary names a rewrite tries after the unnumbered one.
///
/// The base name already contains the process id, so a collision means two rewrites of the same
/// file are in flight inside one process. Sixteen retries settle that and nothing else; an
/// unbounded search would turn a wedged directory into a hang.
const PROCEDURE_TEMP_NAME_RETRIES: u32 = 16;

/// Replaces one procedure file with `document`, atomically and in place.
///
/// The sequence is the standard durable replace, with every step named relative to the directory
/// descriptor the read already holds: stage the bytes in a sibling temporary file, `fchmod` it to
/// the original's permissions, flush it to the device, rename it over the target, then flush the
/// directory entry. A reader of the target therefore sees either the whole old document or the
/// whole new one, never a truncated file, and the rename can only ever affect the one name the
/// request already resolved.
///
/// Any failure before the rename removes the temporary file and leaves the original untouched.
pub(super) fn replace_procedure_document(
    opened: &OpenedProcedure,
    document: &[u8],
) -> Result<(), LocalFailure> {
    let (name, file) = create_procedure_temp(opened)?;
    let staged = write_procedure_temp(file, opened.mode, document).and_then(|()| {
        renameat(
            &opened.parent,
            name.as_os_str(),
            &opened.parent,
            opened.leaf.as_os_str(),
        )
        .map_err(|_| procedure_write_failure())
    });
    if let Err(failure) = staged {
        // Best effort by definition: the write already failed, and a leftover temporary file is a
        // smaller problem than reporting a second failure about the cleanup of the first.
        let _ = unlinkat(&opened.parent, name.as_os_str(), UnlinkatFlags::NoRemoveDir);
        return Err(failure);
    }
    // The rename is only durable once the directory entry is. Reporting a failure here is honest
    // even though the content is already in place: re-running `--write` is idempotent and will
    // report the file as already canonical.
    fsync(&opened.parent).map_err(|_| procedure_write_failure())
}

/// Creates the sibling temporary file a rewrite stages into, and returns its name.
///
/// `O_EXCL` means an existing entry is never clobbered and `O_NOFOLLOW` means a symlink planted
/// under the temporary name is never followed, so this can only ever create a fresh regular file
/// next to the target. The leading `.` keeps a half-written document out of an ordinary listing.
fn create_procedure_temp(opened: &OpenedProcedure) -> Result<(OsString, fs::File), LocalFailure> {
    let mut base = OsString::from(".");
    base.push(&opened.leaf);
    base.push(format!(".{}.podway-tmp", std::process::id()));
    let numbered = (0..PROCEDURE_TEMP_NAME_RETRIES).map(|attempt| {
        let mut name = base.clone();
        name.push(format!(".{attempt}"));
        name
    });
    for name in std::iter::once(base.clone()).chain(numbered) {
        match openat(
            &opened.parent,
            name.as_os_str(),
            OFlag::O_CLOEXEC | OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_NOFOLLOW | OFlag::O_WRONLY,
            opened.mode,
        ) {
            Ok(descriptor) => return Ok((name, fs::File::from(descriptor))),
            Err(Errno::EEXIST) => {}
            Err(_) => return Err(procedure_write_failure()),
        }
    }
    Err(procedure_write_failure())
}

/// Fills the staged temporary file and makes its bytes durable.
fn write_procedure_temp(
    mut file: fs::File,
    mode: Mode,
    document: &[u8],
) -> Result<(), LocalFailure> {
    // `openat`'s mode argument is filtered by the process umask; `fchmod` is not. Without this a
    // `0o664` procedure rewritten under a `0o022` umask would come back `0o644`.
    fchmod(&file, mode).map_err(|_| procedure_write_failure())?;
    file.write_all(document)
        .map_err(|_| procedure_write_failure())?;
    // Flushing before the rename is what makes the replace atomic against a crash rather than only
    // against a concurrent reader: the name never points at bytes that are not on the device.
    file.sync_all().map_err(|_| procedure_write_failure())
}

/// The failure a rewrite reports when the operating system refuses an I/O operation.
///
/// `INTERNAL_ERROR` is the code this CLI already uses for a local I/O failure that says nothing
/// about the request — `render_write_failure` reports a failed stdout write through the same code.
/// Every procedure code would be a false statement here: the document was found, is valid, and is
/// renderable, and the only thing that went wrong is that the filesystem would not take it.
fn procedure_write_failure() -> LocalFailure {
    LocalFailure::catalog(
        "INTERNAL_ERROR",
        "cannot write the procedure file",
        PROCEDURE_FORMAT_COMMAND,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    struct OwnedDirectory(PathBuf);
    impl Drop for OwnedDirectory {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn bound005_failed_replace_preserves_the_destination_and_removes_staging() {
        let root = std::env::temp_dir().join(format!("podway-format-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let owned = OwnedDirectory(root);
        let file = owned.0.join("procedure.yaml");
        fs::write(&file, b"original source").unwrap();
        let opened = open_offline_procedure(&file).unwrap();
        fs::rename(&file, owned.0.join("parked.yaml")).unwrap();
        fs::create_dir(&file).unwrap();
        fs::write(file.join("sentinel"), b"preserve destination").unwrap();
        let failure = replace_procedure_document(&opened, b"replacement source").unwrap_err();
        assert_eq!(failure.code, "INTERNAL_ERROR");
        assert_eq!(
            fs::read(owned.0.join("parked.yaml")).unwrap(),
            b"original source"
        );
        assert_eq!(
            fs::read(file.join("sentinel")).unwrap(),
            b"preserve destination"
        );
        assert!(fs::read_dir(&owned.0).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .contains(".podway-tmp")
        }));
    }
}
