//! Tracked config initialization uses opaque bytes and descriptor-relative placement.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::ffi::OsString;
use std::fs;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::os::unix::fs::{FileTypeExt, PermissionsExt, symlink};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};
use std::process::Command;

use podway_git::{
    DiagnosticPathDisplayV1, GitResolverContractV1, LosslessPathV1, NativeGitResolverV1,
    ValidatedWorktreeV1, WORKTREE_SELECTOR_VERSION_V1, WorkspaceLayoutElementStatusV1,
    WorkspaceLayoutInitializerV1, WorktreeSelectorV1,
};
use tempfile::TempDir;

fn lossless(path: &Path) -> LosslessPathV1 {
    let display = DiagnosticPathDisplayV1::new("fixture").expect("fixture display");
    LosslessPathV1::from_raw_bytes(path.as_os_str().as_bytes(), display)
        .expect("temporary fixture path is absolute and canonical")
}

fn resolve(path: &Path) -> ValidatedWorktreeV1 {
    let path = fs::canonicalize(path).expect("canonical fixture selection");
    NativeGitResolverV1::new()
        .resolve(
            WorktreeSelectorV1::new(WORKTREE_SELECTOR_VERSION_V1, None, lossless(&path))
                .expect("fixture selector"),
        )
        .expect("fixture worktree resolves")
}

fn create_main(root: &Path) {
    let administration = root.join(".git");
    fs::create_dir_all(administration.join("objects")).expect("objects directory");
    fs::create_dir_all(administration.join("refs")).expect("refs directory");
    fs::write(administration.join("HEAD"), b"ref: refs/heads/main\n").expect("HEAD record");
}

fn temporary() -> TempDir {
    tempfile::tempdir().expect("temporary directory")
}
#[derive(Debug, Eq, PartialEq)]
struct GitTreeNodeV1 {
    kind: GitTreeNodeKindV1,
    mode: u32,
    bytes: Option<Vec<u8>>,
    symlink_target: Option<Vec<u8>>,
}

#[derive(Debug, Eq, PartialEq)]
enum GitTreeNodeKindV1 {
    Directory,
    RegularFile,
    Symlink,
    BlockDevice,
    CharacterDevice,
    Fifo,
    Socket,
}

fn git_tree_metadata(path: &Path) -> BTreeMap<PathBuf, GitTreeNodeV1> {
    fn node_kind(file_type: fs::FileType, path: &Path) -> GitTreeNodeKindV1 {
        if file_type.is_dir() {
            GitTreeNodeKindV1::Directory
        } else if file_type.is_file() {
            GitTreeNodeKindV1::RegularFile
        } else if file_type.is_symlink() {
            GitTreeNodeKindV1::Symlink
        } else if file_type.is_block_device() {
            GitTreeNodeKindV1::BlockDevice
        } else if file_type.is_char_device() {
            GitTreeNodeKindV1::CharacterDevice
        } else if file_type.is_fifo() {
            GitTreeNodeKindV1::Fifo
        } else if file_type.is_socket() {
            GitTreeNodeKindV1::Socket
        } else {
            panic!(
                "Git administration contains an unsupported node type: {}",
                path.display()
            );
        }
    }

    fn collect(root: &Path, path: &Path, output: &mut BTreeMap<PathBuf, GitTreeNodeV1>) {
        let metadata = fs::symlink_metadata(path).expect("Git administration metadata");
        let file_type = metadata.file_type();
        let kind = node_kind(file_type, path);
        let relative = path
            .strip_prefix(root)
            .expect("administration child")
            .to_path_buf();
        let node = GitTreeNodeV1 {
            mode: metadata.permissions().mode() & 0o7777,
            bytes: file_type
                .is_file()
                .then(|| fs::read(path).expect("administration bytes")),
            symlink_target: file_type.is_symlink().then(|| {
                fs::read_link(path)
                    .expect("administration symlink target")
                    .into_os_string()
                    .as_bytes()
                    .to_vec()
            }),
            kind,
        };
        let is_directory = matches!(node.kind, GitTreeNodeKindV1::Directory);
        output.insert(relative, node);
        if is_directory {
            for entry in fs::read_dir(path).expect("administration directory") {
                collect(root, &entry.expect("administration entry").path(), output);
            }
        }
    }

    let mut output = BTreeMap::new();
    collect(path, path, &mut output);
    output
}
fn linked_administration_root(linked_worktree: &Path) -> PathBuf {
    let marker = linked_worktree.join(".git");
    let marker_bytes = fs::read(&marker).expect("linked Git marker bytes");
    let gitdir = marker_bytes
        .strip_prefix(b"gitdir: ")
        .and_then(|value| value.strip_suffix(b"\n"))
        .expect("linked Git marker must contain one gitdir path");
    let gitdir = PathBuf::from(OsString::from_vec(gitdir.to_vec()));
    let gitdir = if gitdir.is_absolute() {
        gitdir
    } else {
        linked_worktree.join(gitdir)
    };
    fs::canonicalize(gitdir).expect("linked Git administration root")
}

fn assert_git_tree_unchanged(
    expected: &BTreeMap<PathBuf, GitTreeNodeV1>,
    administration: &Path,
    message: &str,
) {
    assert_eq!(&git_tree_metadata(administration), expected, "{message}");
}

fn assert_no_layout_temporary_files(podway: &Path) {
    for entry in fs::read_dir(podway).expect("podway directory") {
        let name = entry.expect("podway entry").file_name();
        let bytes = name.as_os_str().as_bytes();
        assert!(
            !bytes.starts_with(b".podway-ignore-") && !bytes.starts_with(b".podway-config-"),
            "retained temporary layout entry: {name:?}"
        );
    }
}

#[test]
fn absent_config_uses_exact_opaque_bytes_and_preserves_git_administration() {
    let temporary = temporary();
    let worktree = temporary.path().join("worktree");
    create_main(&worktree);
    let validated = resolve(&worktree);
    let admin_before = git_tree_metadata(&worktree.join(".git"));
    let default = vec![b'\xff'; 64 * 1024];

    let first = WorkspaceLayoutInitializerV1::new()
        .initialize_with_config(&validated, &default)
        .expect("first initialization");
    assert_eq!(
        first.config_file(),
        Some(WorkspaceLayoutElementStatusV1::Created)
    );
    assert_eq!(
        fs::read(worktree.join(".podway/config.yaml")).expect("created config"),
        default
    );
    assert_no_layout_temporary_files(&worktree.join(".podway"));

    let replay = WorkspaceLayoutInitializerV1::new()
        .initialize_with_config(&validated, b"different opaque defaults")
        .expect("replayed initialization");
    assert_eq!(
        replay.config_file(),
        Some(WorkspaceLayoutElementStatusV1::AlreadyValid)
    );
    assert_eq!(
        fs::read(worktree.join(".podway/config.yaml")).expect("replayed config"),
        default
    );
    assert_no_layout_temporary_files(&worktree.join(".podway"));
    assert_eq!(admin_before, git_tree_metadata(&worktree.join(".git")));
}
#[test]
fn pac036_runtime_is_confined_to_podway_and_ignored_by_the_exact_rule() {
    let temporary = temporary();
    let worktree = temporary.path().join("worktree");
    create_main(&worktree);
    let validated = resolve(&worktree);

    let report = WorkspaceLayoutInitializerV1::new()
        .initialize_with_config(&validated, b"schema: podway.procedure/v1\n")
        .expect("layout initialization");
    let podway = worktree.join(".podway");
    let runtime = podway.join("runtime");
    assert!(
        runtime.is_dir(),
        "runtime state must be below .podway/runtime"
    );
    assert_eq!(
        report.runtime_directory(),
        WorkspaceLayoutElementStatusV1::Created
    );
    assert_eq!(
        fs::read(podway.join(".gitignore")).expect("runtime ignore rule"),
        b"runtime/\n",
        "the workspace-local ignore file must contain exactly the runtime directory rule"
    );
    assert!(
        !worktree.join("runtime").exists(),
        "runtime state must not be placed at the worktree root"
    );

    let replay = WorkspaceLayoutInitializerV1::new()
        .initialize_with_config(&validated, b"different default")
        .expect("idempotent layout replay");
    assert_eq!(
        replay.runtime_directory(),
        WorkspaceLayoutElementStatusV1::AlreadyValid
    );
    assert_eq!(
        fs::read(podway.join(".gitignore")).expect("replayed runtime ignore rule"),
        b"runtime/\n",
        "replay must neither broaden nor duplicate the exact ignore rule"
    );
}

#[test]
fn pac062_layout_api_preserves_real_main_and_linked_worktree_metadata() {
    let temporary = temporary();
    let main = temporary.path().join("main");
    let linked = temporary.path().join("linked");
    assert!(
        Command::new("git")
            .args(["init", "--initial-branch=main"])
            .arg(&main)
            .status()
            .expect("Git capability seam must start a representative main worktree")
            .success()
    );
    for (key, value) in [
        ("user.email", "pac062@example.invalid"),
        ("user.name", "PAC-062"),
    ] {
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(&main)
                .args(["config", key, value])
                .status()
                .expect("Git capability seam must configure fixture identity")
                .success()
        );
    }
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&main)
            .args(["commit", "--allow-empty", "-m", "fixture"])
            .status()
            .expect("Git capability seam must create fixture commit")
            .success()
    );
    assert!(
        Command::new("git")
            .arg("-C")
            .arg(&main)
            .args(["worktree", "add"])
            .arg(&linked)
            .status()
            .expect("Git capability seam must create representative linked worktree")
            .success()
    );

    let linked_administration = linked_administration_root(&linked);

    let main_administration_before_main_resolve = git_tree_metadata(&main.join(".git"));
    let linked_marker_before_main_resolve = git_tree_metadata(&linked.join(".git"));
    let linked_administration_before_main_resolve = git_tree_metadata(&linked_administration);
    let main_validated = resolve(&main);
    assert_git_tree_unchanged(
        &main_administration_before_main_resolve,
        &main.join(".git"),
        "the public main-worktree resolver must not mutate Git administration metadata",
    );
    assert_git_tree_unchanged(
        &linked_marker_before_main_resolve,
        &linked.join(".git"),
        "the public main-worktree resolver must not mutate the linked Git marker",
    );
    assert_git_tree_unchanged(
        &linked_administration_before_main_resolve,
        &linked_administration,
        "the public main-worktree resolver must not mutate linked Git administration metadata",
    );

    let main_administration_before_linked_resolve = git_tree_metadata(&main.join(".git"));
    let linked_marker_before_linked_resolve = git_tree_metadata(&linked.join(".git"));
    let linked_administration_before_linked_resolve = git_tree_metadata(&linked_administration);
    let linked_validated = resolve(&linked);
    assert_git_tree_unchanged(
        &main_administration_before_linked_resolve,
        &main.join(".git"),
        "the public linked-worktree resolver must not mutate Git administration metadata",
    );
    assert_git_tree_unchanged(
        &linked_marker_before_linked_resolve,
        &linked.join(".git"),
        "the public linked-worktree resolver must not mutate the linked Git marker",
    );
    assert_git_tree_unchanged(
        &linked_administration_before_linked_resolve,
        &linked_administration,
        "the public linked-worktree resolver must not mutate linked Git administration metadata",
    );

    let main_administration_before_main_layout = git_tree_metadata(&main.join(".git"));
    let linked_marker_before_main_layout = git_tree_metadata(&linked.join(".git"));
    let linked_administration_before_main_layout = git_tree_metadata(&linked_administration);
    WorkspaceLayoutInitializerV1::new()
        .initialize_with_config(&main_validated, b"opaque config")
        .expect("public layout API");
    assert_git_tree_unchanged(
        &main_administration_before_main_layout,
        &main.join(".git"),
        "the public main-worktree layout API must not mutate Git administration metadata",
    );
    assert_git_tree_unchanged(
        &linked_marker_before_main_layout,
        &linked.join(".git"),
        "the public main-worktree layout API must not mutate the linked Git marker",
    );
    assert_git_tree_unchanged(
        &linked_administration_before_main_layout,
        &linked_administration,
        "the public main-worktree layout API must not mutate linked Git administration metadata",
    );

    let main_administration_before_linked_layout = git_tree_metadata(&main.join(".git"));
    let linked_marker_before_linked_layout = git_tree_metadata(&linked.join(".git"));
    let linked_administration_before_linked_layout = git_tree_metadata(&linked_administration);
    WorkspaceLayoutInitializerV1::new()
        .initialize_with_config(&linked_validated, b"opaque config")
        .expect("public layout API");
    assert_git_tree_unchanged(
        &main_administration_before_linked_layout,
        &main.join(".git"),
        "the public linked-worktree layout API must not mutate Git administration metadata",
    );
    assert_git_tree_unchanged(
        &linked_marker_before_linked_layout,
        &linked.join(".git"),
        "the public linked-worktree layout API must not mutate the linked Git marker",
    );
    assert_git_tree_unchanged(
        &linked_administration_before_linked_layout,
        &linked_administration,
        "the public linked-worktree layout API must not mutate linked Git administration metadata",
    );
    assert_git_public_api_and_dependency_policy();
}

#[test]
fn existing_custom_config_bytes_and_permissions_are_preserved_idempotently() {
    let temporary = temporary();
    let worktree = temporary.path().join("worktree");
    create_main(&worktree);
    let podway = worktree.join(".podway");
    fs::create_dir(&podway).expect("podway directory");
    let config = podway.join("config.yaml");
    let custom = b"not yaml \xff [preserve exactly]";
    fs::write(&config, custom).expect("custom config");
    fs::set_permissions(&config, fs::Permissions::from_mode(0o640)).expect("custom mode");
    let validated = resolve(&worktree);

    let defaults: [&[u8]; 2] = [b"first default", b"second default"];
    for default in defaults {
        let report = WorkspaceLayoutInitializerV1::new()
            .initialize_with_config(&validated, default)
            .expect("initialize existing config");
        assert_eq!(
            report.config_file(),
            Some(WorkspaceLayoutElementStatusV1::AlreadyValid)
        );
        assert_eq!(fs::read(&config).expect("custom config bytes"), custom);
        assert_eq!(
            fs::metadata(&config)
                .expect("custom config metadata")
                .permissions()
                .mode()
                & 0o777,
            0o640
        );
    }
    assert_no_layout_temporary_files(&podway);
}

#[test]
fn invalid_default_config_bytes_fail_before_layout_mutation() {
    for default in [Vec::new(), vec![b'x'; 64 * 1024 + 1]] {
        let temporary = temporary();
        let worktree = temporary.path().join("worktree");
        create_main(&worktree);
        let validated = resolve(&worktree);
        let admin_before = git_tree_metadata(&worktree.join(".git"));

        assert!(
            WorkspaceLayoutInitializerV1::new()
                .initialize_with_config(&validated, &default)
                .is_err()
        );
        assert!(!worktree.join(".podway").exists());
        assert_eq!(admin_before, git_tree_metadata(&worktree.join(".git")));
    }
}

#[test]
fn config_conflicting_node_kinds_fail_without_layout_mutation() {
    for kind in ["symlink", "directory", "fifo", "socket"] {
        let temporary = temporary();
        let worktree = temporary.path().join(kind);
        create_main(&worktree);
        let podway = worktree.join(".podway");
        fs::create_dir(&podway).expect("podway directory");
        let config = podway.join("config.yaml");
        let outside = worktree.join("outside-config");
        let _socket = match kind {
            "symlink" => {
                fs::write(&outside, b"outside config").expect("outside config");
                symlink(&outside, &config).expect("config symlink");
                None
            }
            "directory" => {
                fs::create_dir(&config).expect("config directory");
                None
            }
            "fifo" => {
                let status = Command::new("mkfifo")
                    .arg(&config)
                    .status()
                    .expect("mkfifo must be available on Unix");
                assert!(status.success(), "mkfifo must create config fixture");
                None
            }
            "socket" => Some(UnixListener::bind(&config).expect("config socket")),
            _ => unreachable!("fixed node kind"),
        };
        let validated = resolve(&worktree);

        assert!(
            WorkspaceLayoutInitializerV1::new()
                .initialize_with_config(&validated, b"opaque default")
                .is_err(),
            "{kind} must be rejected"
        );
        assert!(!podway.join("procedures").exists());
        assert!(!podway.join("runtime").exists());
        assert!(!podway.join(".gitignore").exists());
        let file_type = fs::symlink_metadata(&config)
            .expect("conflicting config entry")
            .file_type();
        match kind {
            "symlink" => {
                assert!(file_type.is_symlink());
                assert_eq!(
                    fs::read(&outside).expect("outside config"),
                    b"outside config"
                );
            }
            "directory" => assert!(file_type.is_dir()),
            "fifo" => assert!(file_type.is_fifo()),
            "socket" => assert!(file_type.is_socket()),
            _ => unreachable!("fixed node kind"),
        }
        assert_no_layout_temporary_files(&podway);
    }
}
#[derive(Debug)]
enum CargoMetadataValueV1 {
    Array(Vec<CargoMetadataValueV1>),
    Object(BTreeMap<String, CargoMetadataValueV1>),
    String(String),
    Scalar,
}

impl CargoMetadataValueV1 {
    fn object(&self, context: &str) -> &BTreeMap<String, Self> {
        match self {
            Self::Object(value) => value,
            _ => panic!("{context} must be a JSON object"),
        }
    }

    fn array(&self, context: &str) -> &[Self] {
        match self {
            Self::Array(value) => value,
            _ => panic!("{context} must be a JSON array"),
        }
    }

    fn string(&self, context: &str) -> &str {
        match self {
            Self::String(value) => value,
            _ => panic!("{context} must be a JSON string"),
        }
    }
}

fn parse_cargo_metadata_value_v1(input: &[u8]) -> CargoMetadataValueV1 {
    fn whitespace(input: &[u8], cursor: &mut usize) {
        while input.get(*cursor).is_some_and(u8::is_ascii_whitespace) {
            *cursor += 1;
        }
    }

    fn string(input: &[u8], cursor: &mut usize) -> String {
        assert_eq!(input.get(*cursor), Some(&b'"'), "JSON string must open");
        *cursor += 1;
        let mut output = String::new();
        while let Some(&byte) = input.get(*cursor) {
            *cursor += 1;
            match byte {
                b'"' => return output,
                b'\\' => {
                    let escaped = *input.get(*cursor).expect("JSON escape");
                    *cursor += 1;
                    match escaped {
                        b'"' => output.push('"'),
                        b'\\' => output.push('\\'),
                        b'/' => output.push('/'),
                        b'b' => output.push('\u{0008}'),
                        b'f' => output.push('\u{000C}'),
                        b'n' => output.push('\n'),
                        b'r' => output.push('\r'),
                        b't' => output.push('\t'),
                        b'u' => {
                            let digits = std::str::from_utf8(
                                input
                                    .get(*cursor..*cursor + 4)
                                    .expect("JSON unicode escape"),
                            )
                            .expect("JSON unicode digits");
                            let codepoint =
                                u32::from_str_radix(digits, 16).expect("JSON unicode codepoint");
                            output.push(
                                char::from_u32(codepoint).expect("JSON unicode scalar value"),
                            );
                            *cursor += 4;
                        }
                        _ => panic!("invalid JSON escape"),
                    }
                }
                byte if byte.is_ascii_control() => panic!("control byte in JSON string"),
                _ => {
                    let start = *cursor - 1;
                    let character = std::str::from_utf8(&input[start..])
                        .expect("Cargo metadata strings must be UTF-8")
                        .chars()
                        .next()
                        .expect("JSON string character");
                    output.push(character);
                    *cursor += character.len_utf8() - 1;
                }
            }
        }
        panic!("unterminated JSON string");
    }

    fn value(input: &[u8], cursor: &mut usize) -> CargoMetadataValueV1 {
        whitespace(input, cursor);
        match input.get(*cursor).copied().expect("JSON value") {
            b'{' => {
                *cursor += 1;
                let mut object = BTreeMap::new();
                whitespace(input, cursor);
                while input.get(*cursor) != Some(&b'}') {
                    let key = string(input, cursor);
                    whitespace(input, cursor);
                    assert_eq!(input.get(*cursor), Some(&b':'), "JSON object separator");
                    *cursor += 1;
                    let previous = object.insert(key, value(input, cursor));
                    assert!(
                        previous.is_none(),
                        "Cargo metadata JSON object has duplicate key"
                    );
                    whitespace(input, cursor);
                    match input.get(*cursor) {
                        Some(b',') => {
                            *cursor += 1;
                            whitespace(input, cursor);
                        }
                        Some(b'}') => {}
                        _ => panic!("JSON object terminator"),
                    }
                }
                *cursor += 1;
                CargoMetadataValueV1::Object(object)
            }
            b'[' => {
                *cursor += 1;
                let mut array = Vec::new();
                whitespace(input, cursor);
                while input.get(*cursor) != Some(&b']') {
                    array.push(value(input, cursor));
                    whitespace(input, cursor);
                    match input.get(*cursor) {
                        Some(b',') => {
                            *cursor += 1;
                            whitespace(input, cursor);
                        }
                        Some(b']') => {}
                        _ => panic!("JSON array terminator"),
                    }
                }
                *cursor += 1;
                CargoMetadataValueV1::Array(array)
            }
            b'"' => CargoMetadataValueV1::String(string(input, cursor)),
            _ => {
                while input
                    .get(*cursor)
                    .is_some_and(|byte| !byte.is_ascii_whitespace() && !b",]}".contains(byte))
                {
                    *cursor += 1;
                }
                CargoMetadataValueV1::Scalar
            }
        }
    }

    let mut cursor = 0;
    let parsed = value(input, &mut cursor);
    whitespace(input, &mut cursor);
    assert_eq!(cursor, input.len(), "trailing Cargo metadata JSON");
    parsed
}

fn metadata_field<'a>(
    object: &'a BTreeMap<String, CargoMetadataValueV1>,
    field: &str,
) -> &'a CargoMetadataValueV1 {
    object
        .get(field)
        .unwrap_or_else(|| panic!("Cargo metadata must include {field}"))
}

fn assert_git_public_api_and_dependency_policy() {
    let crate_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let metadata = Command::new(env!("CARGO"))
        .arg("metadata")
        .arg("--format-version=1")
        .arg("--filter-platform=aarch64-apple-darwin")
        .arg("--manifest-path")
        .arg(crate_root.join("Cargo.toml"))
        .output()
        .expect("Cargo metadata capability seam must be available");
    assert!(
        metadata.status.success(),
        "Cargo metadata capability seam failed: {}",
        String::from_utf8_lossy(&metadata.stderr)
    );

    let metadata = parse_cargo_metadata_value_v1(&metadata.stdout);
    let metadata = metadata.object("Cargo metadata");
    let manifest_path = crate_root
        .join("Cargo.toml")
        .canonicalize()
        .expect("Git crate manifest canonical path")
        .to_string_lossy()
        .into_owned();
    let packages = metadata_field(metadata, "packages").array("Cargo metadata packages");
    let selected = packages
        .iter()
        .map(|package| package.object("Cargo metadata package"))
        .filter(|package| {
            metadata_field(package, "name").string("package name") == "podway-git"
                && metadata_field(package, "manifest_path").string("package manifest path")
                    == manifest_path
        })
        .collect::<Vec<_>>();
    assert_eq!(
        selected.len(),
        1,
        "Cargo metadata must select exactly the podway-git package by name and manifest identity"
    );
    let root_id = metadata_field(selected[0], "id")
        .string("selected package ID")
        .to_owned();

    let package_by_id = packages
        .iter()
        .map(|package| package.object("Cargo metadata package"))
        .map(|package| {
            (
                metadata_field(package, "id")
                    .string("package ID")
                    .to_owned(),
                package,
            )
        })
        .collect::<BTreeMap<_, _>>();
    let nodes = metadata_field(
        metadata_field(metadata, "resolve").object("Cargo resolve"),
        "nodes",
    )
    .array("Cargo resolve nodes");
    let node_by_id = nodes
        .iter()
        .map(|node| node.object("Cargo resolve node"))
        .map(|node| {
            (
                metadata_field(node, "id")
                    .string("resolve node ID")
                    .to_owned(),
                node,
            )
        })
        .collect::<BTreeMap<_, _>>();

    let mut pending = VecDeque::from([root_id]);
    let mut visited = BTreeSet::new();
    while let Some(package_id) = pending.pop_front() {
        if !visited.insert(package_id.clone()) {
            continue;
        }
        let package = package_by_id
            .get(&package_id)
            .unwrap_or_else(|| panic!("resolved package {package_id} must be present"));
        let package_name = metadata_field(package, "name").string("resolved package name");
        let package_source = package
            .get("source")
            .and_then(|source| match source {
                CargoMetadataValueV1::String(source) => Some(source.as_str()),
                CargoMetadataValueV1::Scalar => None,
                _ => panic!("resolved package source must be a string or null"),
            })
            .unwrap_or("path");
        assert!(
            !["git2", "libgit2", "gix"].contains(&package_name),
            "resolved aarch64-apple-darwin dependency graph must reject forbidden Git wrapper package {package_name} from {package_source}"
        );

        let node = node_by_id
            .get(&package_id)
            .unwrap_or_else(|| panic!("resolved package {package_id} must have a resolve node"));
        for dependency in metadata_field(node, "deps").array("resolve node dependencies") {
            let dependency = dependency.object("resolve dependency");
            pending.push_back(
                metadata_field(dependency, "pkg")
                    .string("resolve dependency package ID")
                    .to_owned(),
            );
        }
    }
}
