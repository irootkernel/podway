use podway_config::IntegrationSettingsSourceV1 as Source;
use podway_engine::integration_settings::{
    IntegrationSettingsReadErrorV1 as Error, read_integration_settings_v1,
};
use podway_git::{
    DiagnosticPathDisplayV1, GitResolverContractV1, LosslessPathV1, NativeGitResolverV1,
    ValidatedWorktreeV1, WORKTREE_SELECTOR_VERSION_V1, WorktreeSelectorV1,
};
use std::{
    fs,
    os::unix::{ffi::OsStrExt, fs::symlink},
    path::Path,
    process::Command,
};

fn worktree(root: &Path) -> ValidatedWorktreeV1 {
    let root = fs::canonicalize(root).unwrap();
    NativeGitResolverV1::new()
        .resolve(
            WorktreeSelectorV1::new(
                WORKTREE_SELECTOR_VERSION_V1,
                None,
                LosslessPathV1::from_raw_bytes(
                    root.as_os_str().as_bytes(),
                    DiagnosticPathDisplayV1::new("settings fixture").unwrap(),
                )
                .unwrap(),
            )
            .unwrap(),
        )
        .unwrap()
}
fn git() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    assert!(
        Command::new("git")
            .args(["init", "--quiet"])
            .arg(dir.path())
            .status()
            .unwrap()
            .success()
    );
    dir
}
fn project(root: &Path, value: &str) {
    fs::create_dir_all(root.join(".podway")).unwrap();
    fs::write(root.join(".podway/config.yaml"), value).unwrap();
}
fn user(home: &Path, value: &str) {
    fs::create_dir_all(home.join(".config/podway")).unwrap();
    fs::write(home.join(".config/podway/config.yaml"), value).unwrap();
}

#[test]
fn explicit_read_is_store_free_and_preserves_validation_and_value_precedence() {
    let root = git();
    let home = tempfile::tempdir().unwrap();
    let git = worktree(root.path());
    let result = read_integration_settings_v1(&git, Some(home.path().as_os_str())).unwrap();
    assert!(!result.enabled);
    assert_eq!(result.source, Source::Default);
    assert!(!root.path().join(".podway").exists());
    assert!(!home.path().join(".config").exists());
    user(
        home.path(),
        "schema: podway.user-config/v1\nintegrations: {dolgorae: {enabled: true}}\n",
    );
    assert_eq!(
        read_integration_settings_v1(&git, Some(home.path().as_os_str()))
            .unwrap()
            .source,
        Source::User
    );
    project(
        root.path(),
        "schema: podway.workspace/v3\nintegrations: {dolgorae: {enabled: false}}\n",
    );
    let result = read_integration_settings_v1(&git, Some(home.path().as_os_str())).unwrap();
    assert!(!result.enabled);
    assert_eq!(result.source, Source::Project);
    user(home.path(), "invalid");
    assert_eq!(
        read_integration_settings_v1(&git, Some(home.path().as_os_str())),
        Err(Error::UserConfigInvalid)
    );
    project(root.path(), "invalid");
    assert_eq!(
        read_integration_settings_v1(&git, None),
        Err(Error::ProjectConfigInvalid)
    );
    assert!(!root.path().join(".podway/runtime").exists());
}

#[test]
fn explicit_read_requires_valid_home_and_refuses_unsafe_user_paths_without_effects() {
    let root = git();
    let git = worktree(root.path());
    for home in [
        None,
        Some(std::ffi::OsStr::new("")),
        Some(std::ffi::OsStr::new("relative")),
    ] {
        assert_eq!(
            read_integration_settings_v1(&git, home),
            Err(Error::UserConfigInvalid)
        );
    }
    for component in [".config", ".config/podway", ".config/podway/config.yaml"] {
        let home = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        let path = home.path().join(component);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        symlink(external.path(), &path).unwrap();
        assert_eq!(
            read_integration_settings_v1(&git, Some(home.path().as_os_str())),
            Err(Error::UserConfigInvalid)
        );
        assert_eq!(fs::read_dir(external.path()).unwrap().count(), 0);
    }
    let home = tempfile::tempdir().unwrap();
    user(home.path(), "schema: podway.user-config/v1\n");
    let path = home.path().join(".config/podway/config.yaml");
    fs::remove_file(&path).unwrap();
    nix::unistd::mkfifo(&path, nix::sys::stat::Mode::S_IRUSR).unwrap();
    assert_eq!(
        read_integration_settings_v1(&git, Some(home.path().as_os_str())),
        Err(Error::UserConfigInvalid)
    );
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();
    assert_eq!(
        read_integration_settings_v1(&git, Some(home.path().as_os_str())),
        Err(Error::UserConfigInvalid)
    );
}

#[test]
fn selected_home_is_lossless_and_missing_user_directories_inherit() {
    use std::os::unix::ffi::OsStringExt;
    let root = git();
    let git = worktree(root.path());
    let parent = tempfile::tempdir().unwrap();
    let mut home = parent
        .path()
        .join(std::ffi::OsString::from_vec(b"home-\xff".to_vec()));
    match fs::create_dir(&home) {
        Ok(()) => {}
        Err(error) if cfg!(target_os = "macos") && error.raw_os_error() == Some(92) => {
            // Match the native Git fixtures: APFS can refuse non-UTF-8 names.
            assert_eq!(
                read_integration_settings_v1(&git, Some(home.as_os_str())),
                Err(Error::UserConfigInvalid)
            );
            home = parent.path().join("home");
            fs::create_dir(&home).unwrap();
        }
        Err(error) => panic!("cannot create native home fixture: {error}"),
    }
    assert_eq!(
        read_integration_settings_v1(&git, Some(home.as_os_str()))
            .unwrap()
            .source,
        Source::Default
    );
    fs::create_dir(home.join(".config")).unwrap();
    assert_eq!(
        read_integration_settings_v1(&git, Some(home.as_os_str()))
            .unwrap()
            .source,
        Source::Default
    );
    fs::create_dir(home.join(".config/podway")).unwrap();
    assert_eq!(
        read_integration_settings_v1(&git, Some(home.as_os_str()))
            .unwrap()
            .source,
        Source::Default
    );
    user(
        &home,
        "schema: podway.user-config/v1\nintegrations: {dolgorae: {enabled: false}}\n",
    );
    assert_eq!(
        read_integration_settings_v1(&git, Some(home.as_os_str()))
            .unwrap()
            .source,
        Source::User
    );
}

#[test]
fn native_config_reads_reject_unreadable_oversized_and_project_symlink_inputs() {
    use std::os::unix::fs::PermissionsExt;
    let root = git();
    let git = worktree(root.path());
    let home = tempfile::tempdir().unwrap();
    user(home.path(), "schema: podway.user-config/v1\n");
    let file = home.path().join(".config/podway/config.yaml");
    if !nix::unistd::geteuid().is_root() {
        fs::set_permissions(&file, fs::Permissions::from_mode(0o0)).unwrap();
        let result = read_integration_settings_v1(&git, Some(home.path().as_os_str()));
        fs::set_permissions(&file, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(result, Err(Error::UserConfigInvalid));
    }
    fs::write(&file, vec![b' '; 65_537]).unwrap();
    assert_eq!(
        read_integration_settings_v1(&git, Some(home.path().as_os_str())),
        Err(Error::UserConfigInvalid)
    );
    fs::create_dir(root.path().join(".podway")).unwrap();
    symlink(&file, root.path().join(".podway/config.yaml")).unwrap();
    assert_eq!(
        read_integration_settings_v1(&git, None),
        Err(Error::ProjectConfigInvalid)
    );
    assert_eq!(fs::metadata(&file).unwrap().len(), 65_537);
    assert!(!root.path().join(".podway/runtime").exists());
}

#[test]
fn project_root_replaced_after_git_discovery_is_refused_before_settings_read() {
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("worktree");
    fs::create_dir(&root).unwrap();
    assert!(
        Command::new("git")
            .args(["init", "--quiet"])
            .arg(&root)
            .status()
            .unwrap()
            .success()
    );
    let git = worktree(&root);
    fs::rename(&root, parent.path().join("original-worktree")).unwrap();
    fs::create_dir(&root).unwrap();
    project(
        &root,
        "schema: podway.workspace/v3\nintegrations: {dolgorae: {enabled: true}}\n",
    );
    let home = tempfile::tempdir().unwrap();
    assert_eq!(
        read_integration_settings_v1(&git, Some(home.path().as_os_str())),
        Err(Error::ProjectConfigInvalid)
    );
    assert!(!root.join(".podway/runtime").exists());
    assert!(!home.path().join(".config").exists());
}
