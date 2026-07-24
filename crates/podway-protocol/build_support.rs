use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

pub fn git_path(workspace: &Path, name: &str) -> Option<PathBuf> {
    let output = Command::new("git")
        .args(["-C", workspace.to_str()?, "rev-parse", "--git-path", name])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let path = String::from_utf8(output.stdout).ok()?.trim().to_owned();
    if path.is_empty() {
        return None;
    }
    let path = PathBuf::from(path);
    Some(if path.is_absolute() {
        path
    } else {
        workspace.join(path)
    })
}

fn nearest_existing_directory(path: &Path) -> Option<PathBuf> {
    path.parent()
        .and_then(|parent| parent.ancestors().find(|ancestor| ancestor.is_dir()))
        .map(Path::to_path_buf)
}

pub fn git_rerun_paths(workspace: &Path) -> Vec<PathBuf> {
    let Some(head_path) = git_path(workspace, "HEAD") else {
        return Vec::new();
    };
    let mut paths = vec![head_path.clone()];
    if let Ok(head) = fs::read_to_string(&head_path)
        && let Some(reference) = head.trim().strip_prefix("ref: ")
        && let Some(reference_path) = git_path(workspace, reference)
    {
        if reference_path.is_file() {
            paths.push(reference_path);
        } else if let Some(parent) = nearest_existing_directory(&reference_path) {
            paths.push(parent);
        }
    }
    if let Some(packed_refs) = git_path(workspace, "packed-refs")
        && packed_refs.is_file()
    {
        paths.push(packed_refs);
    }
    paths.sort();
    paths.dedup();
    paths
}
