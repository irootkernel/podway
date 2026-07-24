use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

#[path = "../build_support.rs"]
mod build_support;

struct GitFixture {
    root: PathBuf,
}

impl GitFixture {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("fixture clock must follow the Unix epoch")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "podway-protocol-git-refs-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir(&root).expect("fixture repository directory must be created");
        run_git(&root, &["init", "-b", "main"]);
        run_git(&root, &["config", "user.name", "Podway Test"]);
        run_git(&root, &["config", "user.email", "podway@example.invalid"]);
        fs::write(root.join("tracked.txt"), "first\n").expect("fixture file must be written");
        run_git(&root, &["add", "tracked.txt"]);
        run_git(&root, &["commit", "-m", "first"]);
        Self { root }
    }
}

impl Drop for GitFixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).expect("fixture repository must be removed");
    }
}

fn run_git(root: &Path, arguments: &[&str]) -> String {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(arguments)
        .output()
        .expect("fixture git command must run");
    assert!(
        output.status.success(),
        "git {arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("fixture git output must be UTF-8")
        .trim()
        .to_owned()
}

#[test]
fn packed_branch_ref_creation_is_covered_by_a_rerun_path() {
    let fixture = GitFixture::new();
    let initial_commit = run_git(&fixture.root, &["rev-parse", "HEAD"]);
    run_git(&fixture.root, &["pack-refs", "--all", "--prune"]);

    let loose_ref = build_support::git_path(&fixture.root, "refs/heads/main")
        .expect("fixture loose ref path must resolve");
    let packed_refs = build_support::git_path(&fixture.root, "packed-refs")
        .expect("fixture packed refs path must resolve");
    assert!(!loose_ref.exists(), "branch ref must begin packed");
    assert!(packed_refs.is_file(), "packed refs must exist");

    let watched = build_support::git_rerun_paths(&fixture.root);
    assert!(watched.contains(&packed_refs));
    assert!(watched.iter().any(|path| {
        path.is_dir() && loose_ref.starts_with(path) && path != fixture.root.as_path()
    }));

    fs::write(fixture.root.join("tracked.txt"), "second\n")
        .expect("advanced fixture file must be written");
    run_git(&fixture.root, &["add", "tracked.txt"]);
    run_git(&fixture.root, &["commit", "-m", "second"]);
    let advanced_commit = run_git(&fixture.root, &["rev-parse", "HEAD"]);

    assert_ne!(advanced_commit, initial_commit);
    assert!(
        loose_ref.is_file(),
        "advancing a packed branch creates a loose ref"
    );
    assert!(
        watched
            .iter()
            .any(|path| { path == &loose_ref || path.is_dir() && loose_ref.starts_with(path) })
    );
    assert!(
        build_support::git_rerun_paths(&fixture.root).contains(&loose_ref),
        "the next build must watch the newly created loose ref directly"
    );
}
