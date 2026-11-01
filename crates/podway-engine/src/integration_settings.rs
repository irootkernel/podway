//! Explicit Git/config-only settings access. Ordinary Direct resolution never calls this seam.

use std::{ffi::OsStr, fmt, fs, path::Path};

use podway_config::{
    MAX_WORKSPACE_CONFIG_BYTES_V1, ResolvedIntegrationSettingsV1, parse_user_config_v1,
    parse_workspace_config_v1, resolve_integration_settings_v1,
};
use podway_git::{NativeGitResolverV1, ValidatedWorktreeV1};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IntegrationSettingsReadErrorV1 {
    ProjectConfigInvalid,
    UserConfigInvalid,
}

impl fmt::Display for IntegrationSettingsReadErrorV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ProjectConfigInvalid => "Workspace configuration is invalid or unavailable.",
            Self::UserConfigInvalid => "User configuration is invalid or unavailable.",
        })
    }
}

impl std::error::Error for IntegrationSettingsReadErrorV1 {}

/// Read project configuration before inspecting the caller-selected, lossless HOME.
/// No Store, lease, workspace UUID, ambient HOME or external executable is consulted.
pub fn read_integration_settings_v1(
    worktree: &ValidatedWorktreeV1,
    home: Option<&OsStr>,
) -> Result<ResolvedIntegrationSettingsV1, IntegrationSettingsReadErrorV1> {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        let root = worktree
            .roots()
            .worktree_root()
            .decode_path_bytes()
            .map_err(|_| IntegrationSettingsReadErrorV1::ProjectConfigInvalid)?;
        let project = read_optional_file(
            Path::new(OsStr::from_bytes(&root)),
            &[".podway", "config.yaml"],
            Some(worktree),
            || {},
        )
        .map_err(|_| IntegrationSettingsReadErrorV1::ProjectConfigInvalid)?;
        let project = project
            .map(parse_workspace_config_v1)
            .transpose()
            .map_err(|_| IntegrationSettingsReadErrorV1::ProjectConfigInvalid)?;
        let home = home
            .filter(|home| !home.is_empty())
            .ok_or(IntegrationSettingsReadErrorV1::UserConfigInvalid)?;
        let user = read_optional_file(
            Path::new(home),
            &[".config", "podway", "config.yaml"],
            None,
            || {},
        )
        .map_err(|_| IntegrationSettingsReadErrorV1::UserConfigInvalid)?;
        let user = user
            .map(parse_user_config_v1)
            .transpose()
            .map_err(|_| IntegrationSettingsReadErrorV1::UserConfigInvalid)?;
        Ok(resolve_integration_settings_v1(
            project
                .as_ref()
                .and_then(|value| value.integrations.as_ref())
                .and_then(|value| value.dolgorae_enabled()),
            user.as_ref()
                .and_then(|value| value.integrations.as_ref())
                .and_then(|value| value.dolgorae_enabled()),
        ))
    }
    #[cfg(not(unix))]
    {
        let _ = (worktree, home);
        Err(IntegrationSettingsReadErrorV1::UserConfigInvalid)
    }
}

#[cfg(unix)]
fn read_optional_file(
    selected_root: &Path,
    components: &[&str],
    expected_worktree: Option<&ValidatedWorktreeV1>,
    after_read: impl FnOnce(),
) -> Result<Option<Vec<u8>>, ()> {
    use nix::{
        errno::Errno,
        fcntl::{AtFlags, OFlag, open, openat},
        sys::stat::{Mode, SFlag, fstatat},
    };
    use std::{fs::File, io::Read, os::unix::fs::MetadataExt};
    if !selected_root.is_absolute() {
        return Err(());
    }
    let root = fs::canonicalize(selected_root).map_err(|_| ())?;
    let root_file = File::from(
        open(
            &root,
            OFlag::O_CLOEXEC | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_RDONLY,
            Mode::empty(),
        )
        .map_err(|_| ())?,
    );
    if let Some(worktree) = expected_worktree {
        NativeGitResolverV1::new()
            .validate_open_worktree_root(worktree, &root_file)
            .map_err(|_| ())?;
    }
    let mut directories = vec![root_file];
    let (leaf, ancestors) = components.split_last().ok_or(())?;
    let mut missing = None;
    for (index, component) in ancestors.iter().enumerate() {
        let parent = directories.last().ok_or(())?;
        match openat(
            parent,
            *component,
            OFlag::O_CLOEXEC | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_RDONLY,
            Mode::empty(),
        ) {
            Ok(fd) => directories.push(File::from(fd)),
            Err(Errno::ENOENT) => {
                missing = Some((index, *component));
                break;
            }
            Err(_) => return Err(()),
        }
    }
    let mut file_witness = None;
    let bytes = if missing.is_some() {
        None
    } else {
        let parent = directories.last().ok_or(())?;
        match fstatat(parent, *leaf, AtFlags::AT_SYMLINK_NOFOLLOW) {
            Err(Errno::ENOENT) => {
                missing = Some((ancestors.len(), *leaf));
                None
            }
            Err(_) => return Err(()),
            Ok(before) => {
                if SFlag::from_bits_truncate(before.st_mode) & SFlag::S_IFMT != SFlag::S_IFREG {
                    return Err(());
                }
                let file = File::from(
                    openat(
                        parent,
                        *leaf,
                        OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_RDONLY,
                        Mode::empty(),
                    )
                    .map_err(|_| ())?,
                );
                let metadata = file.metadata().map_err(|_| ())?;
                if !metadata.is_file()
                    || !stat_matches_metadata(&before, &metadata)
                    || metadata.len() > MAX_WORKSPACE_CONFIG_BYTES_V1 as u64
                {
                    return Err(());
                }
                let mut bytes = Vec::new();
                (&file)
                    .take(MAX_WORKSPACE_CONFIG_BYTES_V1 as u64 + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|_| ())?;
                if bytes.len() > MAX_WORKSPACE_CONFIG_BYTES_V1 {
                    return Err(());
                }
                file_witness = Some((file, metadata));
                Some(bytes)
            }
        }
    };
    after_read();
    if fs::canonicalize(selected_root).map_err(|_| ())? != root {
        return Err(());
    }
    let root_path_metadata = fs::symlink_metadata(&root).map_err(|_| ())?;
    let root_metadata = directories[0].metadata().map_err(|_| ())?;
    if !root_path_metadata.is_dir()
        || root_path_metadata.dev() != root_metadata.dev()
        || root_path_metadata.ino() != root_metadata.ino()
    {
        return Err(());
    }
    for index in 1..directories.len() {
        let stat = fstatat(
            &directories[index - 1],
            ancestors[index - 1],
            AtFlags::AT_SYMLINK_NOFOLLOW,
        )
        .map_err(|_| ())?;
        let opened = directories[index].metadata().map_err(|_| ())?;
        if SFlag::from_bits_truncate(stat.st_mode) & SFlag::S_IFMT != SFlag::S_IFDIR
            || stat.st_dev as u64 != opened.dev()
            || stat.st_ino != opened.ino()
        {
            return Err(());
        }
    }
    if let Some((index, name)) = missing
        && !matches!(
            fstatat(&directories[index], name, AtFlags::AT_SYMLINK_NOFOLLOW),
            Err(Errno::ENOENT)
        )
    {
        return Err(());
    }
    if let Some((file, before)) = file_witness {
        let after = file.metadata().map_err(|_| ())?;
        let stat = fstatat(
            directories.last().ok_or(())?,
            *leaf,
            AtFlags::AT_SYMLINK_NOFOLLOW,
        )
        .map_err(|_| ())?;
        let unchanged = |a: &fs::Metadata, b: &fs::Metadata| {
            a.dev() == b.dev()
                && a.ino() == b.ino()
                && a.len() == b.len()
                && a.mtime() == b.mtime()
                && a.mtime_nsec() == b.mtime_nsec()
                && a.ctime() == b.ctime()
                && a.ctime_nsec() == b.ctime_nsec()
                && a.mode() == b.mode()
        };
        if !unchanged(&before, &after)
            || !stat_matches_metadata(&stat, &after)
            || SFlag::from_bits_truncate(stat.st_mode) & SFlag::S_IFMT != SFlag::S_IFREG
        {
            return Err(());
        }
    }
    Ok(bytes)
}

#[cfg(unix)]
fn stat_matches_metadata(stat: &nix::sys::stat::FileStat, metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    stat.st_dev as u64 == metadata.dev()
        && stat.st_ino == metadata.ino()
        && stat.st_size >= 0
        && stat.st_size as u64 == metadata.len()
        && stat.st_mtime == metadata.mtime()
        && stat.st_mtime_nsec == metadata.mtime_nsec()
        && stat.st_ctime == metadata.ctime()
        && stat.st_ctime_nsec == metadata.ctime_nsec()
        && u32::from(stat.st_mode) == metadata.mode()
        && stat.st_uid == metadata.uid()
        && stat.st_gid == metadata.gid()
}

#[cfg(all(test, unix))]
mod tests {
    use super::read_optional_file;
    use std::fs;

    #[test]
    fn native_settings_reads_refuse_replaced_roots_ancestors_files_and_absence() {
        for replace in ["root", "ancestor", "file", "content", "missing"] {
            let parent = tempfile::tempdir().unwrap();
            let root = parent.path().join("home");
            fs::create_dir_all(root.join(".config/podway")).unwrap();
            let leaf = root.join(".config/podway/config.yaml");
            if replace != "missing" {
                fs::write(&leaf, b"original").unwrap();
            }
            let result =
                read_optional_file(&root, &[".config", "podway", "config.yaml"], None, || {
                    match replace {
                        "root" => {
                            fs::rename(&root, parent.path().join("retired")).unwrap();
                            fs::create_dir(&root).unwrap();
                        }
                        "ancestor" => {
                            fs::rename(root.join(".config"), root.join("old-config")).unwrap();
                            fs::create_dir(root.join(".config")).unwrap();
                        }
                        "file" => {
                            fs::rename(&leaf, leaf.with_extension("old")).unwrap();
                            fs::write(&leaf, b"original").unwrap();
                        }
                        "content" => fs::write(&leaf, b"revised").unwrap(),
                        "missing" => fs::write(&leaf, b"new").unwrap(),
                        _ => unreachable!(),
                    }
                });
            assert!(result.is_err(), "accepted changed {replace}");
        }
    }
}
