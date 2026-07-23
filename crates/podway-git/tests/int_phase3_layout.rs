//! Worktree-local layout initialization fixtures construct Git metadata without `git`.

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{PermissionsExt, symlink};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Barrier};
use std::thread;

use podway_git::{
    DiagnosticPathDisplayV1, GitResolverContractV1, LosslessPathV1, NativeGitResolverV1,
    ValidatedWorktreeV1, WORKTREE_SELECTOR_VERSION_V1, WorkspaceLayoutElementStatusV1,
    WorkspaceLayoutErrorV1, WorkspaceLayoutInitializerV1, WorktreeKindV1, WorktreeSelectorV1,
};
use tempfile::TempDir;

fn lossless(path: &Path) -> LosslessPathV1 {
    let display = DiagnosticPathDisplayV1::new(path.as_os_str().to_string_lossy().into_owned())
        .expect("temporary fixture display is bounded");
    LosslessPathV1::from_raw_bytes(path.as_os_str().as_bytes(), display)
        .expect("temporary fixture path is absolute and canonical")
}

fn resolve(path: &Path) -> ValidatedWorktreeV1 {
    let path = fs::canonicalize(path).expect("canonical fixture selection");
    NativeGitResolverV1::new()
        .resolve(
            WorktreeSelectorV1::new(WORKTREE_SELECTOR_VERSION_V1, None, lossless(&path))
                .expect("fixture selector is valid"),
        )
        .expect("fixture worktree resolves")
}

fn write_head(directory: &Path) {
    fs::write(directory.join("HEAD"), b"ref: refs/heads/main\n").expect("HEAD record");
}

fn create_common(common: &Path) {
    fs::create_dir_all(common.join("objects")).expect("common objects directory");
    fs::create_dir_all(common.join("refs")).expect("common refs directory");
    fs::create_dir_all(common.join("worktrees")).expect("common worktrees directory");
    write_head(common);
}

fn create_main(root: &Path) {
    create_common(&root.join(".git"));
}

fn create_linked(worktree: &Path, common: &Path, name: &str) {
    create_common(common);
    let administration = common.join("worktrees").join(name);
    fs::create_dir_all(&administration).expect("linked administration directory");
    fs::create_dir_all(worktree).expect("linked worktree directory");

    let administration = fs::canonicalize(&administration).expect("canonical administration");
    let marker = fs::canonicalize(worktree)
        .expect("canonical linked worktree")
        .join(".git");
    fs::write(
        &marker,
        [
            b"gitdir: ".as_slice(),
            administration.as_os_str().as_bytes(),
            b"\n",
        ]
        .concat(),
    )
    .expect("linked Git marker");
    write_linked_administration(&administration, &marker);
}

fn write_linked_administration(administration: &Path, marker: &Path) {
    fs::create_dir_all(administration).expect("linked administration directory");
    write_head(administration);
    fs::write(administration.join("commondir"), b"../..\n").expect("common record");
    fs::write(
        administration.join("gitdir"),
        [marker.as_os_str().as_bytes(), b"\n"].concat(),
    )
    .expect("reciprocal Git marker");
}

fn temporary() -> TempDir {
    tempfile::tempdir().expect("temporary directory")
}

fn initialize(worktree: &ValidatedWorktreeV1) -> podway_git::WorkspaceLayoutReportV1 {
    WorkspaceLayoutInitializerV1::new()
        .initialize(worktree)
        .expect("layout initialization")
}

fn admin_bytes(path: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn collect(root: &Path, path: &Path, output: &mut BTreeMap<PathBuf, Vec<u8>>) {
        if path.is_file() {
            output.insert(
                path.strip_prefix(root).expect("admin child").to_path_buf(),
                fs::read(path).expect("admin bytes"),
            );
            return;
        }
        for entry in fs::read_dir(path).expect("admin directory") {
            collect(root, &entry.expect("admin entry").path(), output);
        }
    }

    let mut output = BTreeMap::new();
    collect(path, path, &mut output);
    output
}
fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).expect("copy destination directory");
    for entry in fs::read_dir(source).expect("copy source directory") {
        let entry = entry.expect("copy source entry");
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if source_path.is_dir() {
            copy_tree(&source_path, &destination_path);
        } else {
            fs::copy(&source_path, &destination_path).expect("copy source file");
        }
    }
}

fn assert_created(report: &podway_git::WorkspaceLayoutReportV1) {
    assert_eq!(
        report.podway_directory(),
        WorkspaceLayoutElementStatusV1::Created
    );
    assert_eq!(
        report.procedures_directory(),
        WorkspaceLayoutElementStatusV1::Created
    );
    assert_eq!(
        report.runtime_directory(),
        WorkspaceLayoutElementStatusV1::Created
    );
    assert_eq!(
        report.gitignore_file(),
        WorkspaceLayoutElementStatusV1::Created
    );
    assert_eq!(
        report.runtime_ignore_rule(),
        WorkspaceLayoutElementStatusV1::Created
    );
}

#[test]
fn main_linked_and_nested_worktrees_receive_only_the_local_layout() {
    let temporary = temporary();
    let main = temporary.path().join("main");
    create_main(&main);
    let nested = main.join("nested").join("child");
    fs::create_dir_all(&nested).expect("nested selection");
    let main_worktree = resolve(&nested);
    assert_eq!(main_worktree.kind(), &WorktreeKindV1::Main);
    assert_created(&initialize(&main_worktree));
    assert!(main.join(".podway/procedures").is_dir());
    assert!(!nested.join(".podway").exists());

    let linked = temporary.path().join("linked");
    let common = temporary.path().join("common.git");
    create_linked(&linked, &common, "linked-layout");
    fs::create_dir_all(linked.join("selected/below")).expect("linked nested selection");
    let linked_worktree = resolve(&linked.join("selected").join("below"));
    assert_eq!(linked_worktree.kind(), &WorktreeKindV1::Linked);
    assert_created(&initialize(&linked_worktree));
    assert!(linked.join(".podway/procedures").is_dir());
    assert!(!common.join(".podway").exists());
}

#[test]
fn first_create_replay_and_git_administration_bytes_are_stable() {
    let temporary = temporary();
    let worktree = temporary.path().join("worktree");
    create_main(&worktree);
    let validated = resolve(&worktree);
    let before = admin_bytes(&worktree.join(".git"));

    let first = initialize(&validated);
    assert_created(&first);
    assert_eq!(
        fs::read(worktree.join(".podway/.gitignore")).expect("initial ignore"),
        b"runtime/\n"
    );

    let replay = initialize(&validated);
    for status in [
        replay.podway_directory(),
        replay.procedures_directory(),
        replay.runtime_directory(),
        replay.gitignore_file(),
        replay.runtime_ignore_rule(),
    ] {
        assert_eq!(status, WorkspaceLayoutElementStatusV1::AlreadyValid);
    }
    assert_eq!(before, admin_bytes(&worktree.join(".git")));
}

#[test]
fn linked_worktree_administration_bytes_are_unchanged() {
    let temporary = temporary();
    let worktree = temporary.path().join("linked");
    let common = temporary.path().join("common.git");
    create_linked(&worktree, &common, "admin-bytes");
    let administration = common.join("worktrees/admin-bytes");
    let common_before = admin_bytes(&common);
    let administration_before = admin_bytes(&administration);
    let marker_before = fs::read(worktree.join(".git")).expect("linked marker");

    initialize(&resolve(&worktree));

    assert_eq!(common_before, admin_bytes(&common));
    assert_eq!(administration_before, admin_bytes(&administration));
    assert_eq!(
        marker_before,
        fs::read(worktree.join(".git")).expect("linked marker")
    );
}

#[test]
fn ignore_updates_preserve_unrelated_bytes_and_missing_final_newline() {
    let temporary = temporary();
    let worktree = temporary.path().join("worktree");
    create_main(&worktree);
    fs::create_dir(worktree.join(".podway")).expect("podway directory");
    fs::write(worktree.join(".podway/.gitignore"), b"keep-this").expect("ignore file");

    let report = initialize(&resolve(&worktree));
    assert_eq!(
        report.gitignore_file(),
        WorkspaceLayoutElementStatusV1::AlreadyValid
    );
    assert_eq!(
        report.runtime_ignore_rule(),
        WorkspaceLayoutElementStatusV1::Created
    );
    assert_eq!(
        fs::read(worktree.join(".podway/.gitignore")).expect("normalized ignore"),
        b"keep-this\nruntime/"
    );
}

#[test]
fn duplicate_and_negated_runtime_rules_normalize_to_one_final_ignore_rule() {
    let temporary = temporary();
    let worktree = temporary.path().join("worktree");
    create_main(&worktree);
    fs::create_dir(worktree.join(".podway")).expect("podway directory");
    fs::write(
        worktree.join(".podway/.gitignore"),
        b"keep\nruntime/\nruntime/\n!runtime/\n",
    )
    .expect("duplicate ignore rules");

    let report = initialize(&resolve(&worktree));
    assert_eq!(
        report.runtime_ignore_rule(),
        WorkspaceLayoutElementStatusV1::Created
    );
    let normalized = fs::read(worktree.join(".podway/.gitignore")).expect("normalized ignore");
    assert_eq!(normalized, b"keep\n!runtime/\nruntime/\n");
    assert_eq!(
        normalized
            .split(|byte| *byte == b'\n')
            .filter(|line| *line == &b"runtime/"[..])
            .count(),
        1
    );
}
#[test]
fn broad_negation_after_runtime_rule_is_normalized_to_a_final_effective_rule() {
    let temporary = temporary();
    let worktree = temporary.path().join("worktree");
    create_main(&worktree);
    fs::create_dir(worktree.join(".podway")).expect("podway directory");
    fs::write(
        worktree.join(".podway/.gitignore"),
        b"keep\r\nruntime/\r\n!*/\r\n",
    )
    .expect("ignore rules");

    initialize(&resolve(&worktree));

    assert_eq!(
        fs::read(worktree.join(".podway/.gitignore")).expect("normalized ignore"),
        b"keep\r\n!*/\r\nruntime/\r\n"
    );
}
#[test]
fn posix_negations_preserve_user_bytes_before_one_final_runtime_rule() {
    let temporary = temporary();
    let worktree = temporary.path().join("worktree");
    create_main(&worktree);
    fs::create_dir(worktree.join(".podway")).expect("podway directory");
    fs::write(
        worktree.join(".podway/.gitignore"),
        b"# retained comment\r\nruntime/\r\n!runtim[[:alpha:]]/\n![[:alpha:]]*/\r\n\xffliteral-rule\n",
    )
    .expect("ignore rules");

    let validated = resolve(&worktree);
    let report = initialize(&validated);
    let expected =
        b"# retained comment\r\n!runtim[[:alpha:]]/\n![[:alpha:]]*/\r\n\xffliteral-rule\nruntime/\n";
    let normalized = fs::read(worktree.join(".podway/.gitignore")).expect("normalized ignore");
    assert_eq!(
        report.runtime_ignore_rule(),
        WorkspaceLayoutElementStatusV1::Created
    );
    assert_eq!(normalized, expected);
    assert_eq!(
        normalized
            .split(|byte| *byte == b'\n')
            .filter(|line| *line == &b"runtime/"[..])
            .count(),
        1
    );
    assert!(normalized.ends_with(b"runtime/\n"));

    let replay = initialize(&validated);
    assert_eq!(
        replay.runtime_ignore_rule(),
        WorkspaceLayoutElementStatusV1::AlreadyValid
    );
    assert_eq!(
        fs::read(worktree.join(".podway/.gitignore")).expect("replayed ignore"),
        expected
    );
}

#[test]
fn one_mebibyte_ignore_normalization_is_bounded_and_linear() {
    let temporary = temporary();
    let worktree = temporary.path().join("worktree");
    create_main(&worktree);
    fs::create_dir(worktree.join(".podway")).expect("podway directory");

    let rule_count = (1024 * 1024 - b"!*/\n".len()) / b"runtime/\n".len();
    let mut contents = b"runtime/\n".repeat(rule_count);
    contents.extend_from_slice(b"!*/\n");
    assert_eq!(contents.len(), 1024 * 1024);
    fs::write(worktree.join(".podway/.gitignore"), contents).expect("one mebibyte ignore");

    initialize(&resolve(&worktree));

    assert_eq!(
        fs::read(worktree.join(".podway/.gitignore")).expect("normalized ignore"),
        b"!*/\nruntime/\n"
    );
}
#[test]
fn normalization_growth_over_one_mebibyte_rejects_without_mutation_or_temp_files() {
    let temporary = temporary();
    let worktree = temporary.path().join("worktree");
    create_main(&worktree);
    let podway = worktree.join(".podway");
    fs::create_dir(&podway).expect("podway directory");

    let mut original = b"keep\n".repeat((1024 * 1024 - 1) / b"keep\n".len());
    original.push(b'k');
    assert_eq!(original.len(), 1024 * 1024);
    let ignore = podway.join(".gitignore");
    fs::write(&ignore, &original).expect("one mebibyte source ignore");
    let validated = resolve(&worktree);

    for attempt in 0..2 {
        let result = WorkspaceLayoutInitializerV1::new().initialize(&validated);
        assert!(
            matches!(result, Err(WorkspaceLayoutErrorV1::Initialization { .. })),
            "attempt {attempt} must reject normalized output growth: {result:?}"
        );
        assert_eq!(
            fs::read(&ignore).expect("unchanged source ignore"),
            original,
            "attempt {attempt} changed the source ignore"
        );
        assert!(
            fs::read_dir(&podway)
                .expect("podway directory")
                .all(|entry| {
                    !entry
                        .expect("podway entry")
                        .file_name()
                        .as_os_str()
                        .as_bytes()
                        .starts_with(b".podway-ignore-")
                }),
            "attempt {attempt} retained a staged temporary file"
        );
    }
}

#[test]
fn runtime_permissions_become_exactly_private_and_usable() {
    let temporary = temporary();
    let worktree = temporary.path().join("worktree");
    create_main(&worktree);
    let runtime = worktree.join(".podway/runtime");
    fs::create_dir_all(&runtime).expect("runtime directory");

    for mode in [0o755, 0o400, 0o500, 0o600] {
        fs::set_permissions(&runtime, fs::Permissions::from_mode(mode))
            .expect("runtime permissions");
        initialize(&resolve(&worktree));
        assert_eq!(
            fs::metadata(&runtime)
                .expect("runtime metadata")
                .permissions()
                .mode()
                & 0o777,
            0o700,
            "mode {mode:o} was not repaired to usable private permissions"
        );
    }
}

#[test]
fn symlink_and_file_substitution_at_each_layout_component_fails_closed() {
    let cases = [
        ("podway-symlink", ".podway", true, true),
        ("podway-file", ".podway", false, true),
        ("procedures-symlink", ".podway/procedures", true, false),
        ("procedures-file", ".podway/procedures", false, false),
        ("runtime-symlink", ".podway/runtime", true, true),
        ("runtime-file", ".podway/runtime", false, true),
        ("ignore-symlink", ".podway/.gitignore", true, false),
        ("ignore-directory", ".podway/.gitignore", false, false),
    ];

    for (name, component, use_symlink, revalidation_failure) in cases {
        let temporary = temporary();
        let worktree = temporary.path().join(name);
        create_main(&worktree);
        let validated = resolve(&worktree);
        if component != ".podway" {
            fs::create_dir(worktree.join(".podway")).expect("podway parent");
        }
        let target = worktree.join("outside");
        let target_sentinel = target.join("sentinel");
        fs::create_dir(&target).expect("outside directory");
        fs::write(&target_sentinel, b"outside sentinel").expect("outside sentinel");
        let component = worktree.join(component);
        let component_sentinel = component.join("sentinel");
        if use_symlink {
            symlink(&target, &component).expect("unsafe symlink");
        } else if component.ends_with(".gitignore") {
            fs::create_dir(&component).expect("unsafe directory");
            fs::write(&component_sentinel, b"component sentinel").expect("component sentinel");
        } else {
            fs::write(&component, b"component sentinel").expect("unsafe file");
        }

        let result = WorkspaceLayoutInitializerV1::new().initialize(&validated);
        if revalidation_failure {
            assert!(
                matches!(&result, Err(WorkspaceLayoutErrorV1::Revalidation { .. })),
                "expected revalidation failure, got {result:?}"
            );
        } else {
            assert!(
                matches!(&result, Err(WorkspaceLayoutErrorV1::Initialization { .. })),
                "expected initialization failure, got {result:?}"
            );
        }

        assert_eq!(
            fs::read(&target_sentinel).expect("outside sentinel"),
            b"outside sentinel"
        );
        if use_symlink {
            assert!(
                fs::symlink_metadata(&component)
                    .expect("substituted symlink")
                    .file_type()
                    .is_symlink()
            );
        } else if component.ends_with(".gitignore") {
            assert_eq!(
                fs::read(&component_sentinel).expect("component sentinel"),
                b"component sentinel"
            );
        } else {
            assert_eq!(
                fs::read(&component).expect("substituted component"),
                b"component sentinel"
            );
        }
        assert!(!target.join("procedures").exists());
        assert!(!target.join("runtime").exists());
    }
}

#[test]
fn concurrent_identical_initializers_converge_without_temp_files() {
    let temporary = temporary();
    let worktree = temporary.path().join("worktree");
    create_main(&worktree);
    let validated = resolve(&worktree);
    let barrier = Arc::new(Barrier::new(3));

    let first_worktree = validated.clone();
    let first_barrier = Arc::clone(&barrier);
    let first = thread::spawn(move || {
        first_barrier.wait();
        WorkspaceLayoutInitializerV1::new().initialize(&first_worktree)
    });
    let second_worktree = validated.clone();
    let second_barrier = Arc::clone(&barrier);
    let second = thread::spawn(move || {
        second_barrier.wait();
        WorkspaceLayoutInitializerV1::new().initialize(&second_worktree)
    });
    barrier.wait();

    let first_result = first.join().expect("first initializer thread");
    let second_result = second.join().expect("second initializer thread");
    assert!(
        first_result.is_ok(),
        "first initializer failed: {first_result:#?}"
    );
    assert!(
        second_result.is_ok(),
        "second initializer failed: {second_result:#?}"
    );
    assert_eq!(
        fs::read(worktree.join(".podway/.gitignore")).expect("ignore file"),
        b"runtime/\n"
    );
    let entries: BTreeMap<_, _> = fs::read_dir(worktree.join(".podway"))
        .expect("podway directory")
        .map(|entry| {
            let entry = entry.expect("podway entry");
            let name = entry
                .file_name()
                .into_string()
                .expect("layout entry name is UTF-8");
            let file_type = entry.file_type().expect("layout entry type");
            (name, file_type)
        })
        .collect();
    assert_eq!(
        entries.keys().map(String::as_str).collect::<Vec<_>>(),
        vec![".gitignore", "procedures", "runtime"]
    );
    assert!(entries.get(".gitignore").expect("ignore entry").is_file());
    assert!(
        entries
            .get("procedures")
            .expect("procedures entry")
            .is_dir()
    );
    assert!(entries.get("runtime").expect("runtime entry").is_dir());
}

#[test]
fn stale_copy_move_and_deleted_worktree_identity_fail_before_layout_mutation() {
    let temporary = temporary();
    let worktree = temporary.path().join("worktree");
    create_main(&worktree);
    let validated = resolve(&worktree);
    let moved = temporary.path().join("moved");
    fs::rename(&worktree, &moved).expect("move worktree");
    assert!(
        WorkspaceLayoutInitializerV1::new()
            .initialize(&validated)
            .is_err()
    );
    assert!(!moved.join(".podway").exists());

    let copied = temporary.path().join("copied");
    create_main(&copied);
    let copied_validation = resolve(&copied);
    let displaced = temporary.path().join("displaced-copy-source");
    fs::rename(&copied, &displaced).expect("displace original identity");
    copy_tree(&displaced, &copied);
    assert!(
        WorkspaceLayoutInitializerV1::new()
            .initialize(&copied_validation)
            .is_err()
    );
    assert!(!copied.join(".podway").exists());

    let stale = temporary.path().join("stale");
    create_main(&stale);
    let stale_validation = resolve(&stale);
    fs::remove_dir_all(stale.join(".git")).expect("replace stale administration");
    create_main(&stale);
    let stale_sentinel = stale.join(".git/sentinel");
    fs::write(&stale_sentinel, b"main administration substitution")
        .expect("main administration sentinel");
    let stale_result = WorkspaceLayoutInitializerV1::new().initialize(&stale_validation);
    assert!(
        matches!(
            &stale_result,
            Err(WorkspaceLayoutErrorV1::Revalidation { .. })
        ),
        "expected revalidation failure, got {stale_result:?}"
    );
    assert_eq!(
        fs::read(&stale_sentinel).expect("main administration sentinel"),
        b"main administration substitution"
    );
    assert!(!stale.join(".podway").exists());

    let deleted = temporary.path().join("deleted");
    create_main(&deleted);
    let deleted_validation = resolve(&deleted);
    fs::remove_dir_all(&deleted).expect("delete worktree");
    assert!(
        WorkspaceLayoutInitializerV1::new()
            .initialize(&deleted_validation)
            .is_err()
    );
}
#[test]
fn linked_worktree_metadata_substitutions_fail_revalidation_without_mutation() {
    for substitution in [
        "common",
        "administration",
        "marker",
        "commondir",
        "backlink",
    ] {
        let temporary = temporary();
        let worktree = temporary.path().join("linked");
        let common = temporary.path().join("common.git");
        let name = "stale-linked";
        create_linked(&worktree, &common, name);
        let validated = resolve(&worktree);
        let administration = common.join("worktrees").join(name);
        let marker = fs::canonicalize(&worktree)
            .expect("canonical linked worktree")
            .join(".git");

        let (sentinel, expected) = match substitution {
            "common" => {
                let displaced = temporary.path().join("displaced-common.git");
                fs::rename(&common, displaced).expect("replace common directory");
                create_common(&common);
                write_linked_administration(&administration, &marker);
                let sentinel = common.join("sentinel");
                let expected = b"common substitution".to_vec();
                fs::write(&sentinel, &expected).expect("common sentinel");
                (sentinel, expected)
            }
            "administration" => {
                let displaced = temporary.path().join("displaced-administration");
                fs::rename(&administration, displaced).expect("replace administration directory");
                write_linked_administration(&administration, &marker);
                let sentinel = administration.join("sentinel");
                let expected = b"administration substitution".to_vec();
                fs::write(&sentinel, &expected).expect("administration sentinel");
                (sentinel, expected)
            }
            "marker" => {
                let displaced = temporary.path().join("displaced-marker");
                fs::rename(&marker, displaced).expect("replace linked marker");
                create_common(&marker);
                let sentinel = marker.join("sentinel");
                let expected = b"marker substitution".to_vec();
                fs::write(&sentinel, &expected).expect("marker sentinel");
                (sentinel, expected)
            }
            "commondir" => {
                let sentinel = administration.join("commondir");
                let expected = b"../../missing-common\n".to_vec();
                fs::write(&sentinel, &expected).expect("replace common record");
                (sentinel, expected)
            }
            "backlink" => {
                let sentinel = administration.join("gitdir");
                let expected = temporary
                    .path()
                    .join("missing-marker")
                    .as_os_str()
                    .as_bytes()
                    .iter()
                    .copied()
                    .chain(*b"\n")
                    .collect();
                fs::write(&sentinel, &expected).expect("replace reciprocal marker");
                (sentinel, expected)
            }
            _ => unreachable!("fixed linked substitution fixture"),
        };

        let result = WorkspaceLayoutInitializerV1::new().initialize(&validated);
        assert!(
            matches!(&result, Err(WorkspaceLayoutErrorV1::Revalidation { .. })),
            "{substitution} substitution must fail revalidation, got {result:?}"
        );
        assert_eq!(
            fs::read(&sentinel).expect("substituted sentinel"),
            expected,
            "{substitution} substitution was modified"
        );
        assert!(
            !worktree.join(".podway").exists(),
            "{substitution} substitution started layout mutation"
        );
    }
}
