use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use serde_json::Value;

pub fn canonical_json_bytes(value: &Value) -> Result<Vec<u8>, &'static str> {
    let mut output = Vec::new();
    append_canonical_json(value, &mut output)?;
    Ok(output)
}

fn append_canonical_json(value: &Value, output: &mut Vec<u8>) -> Result<(), &'static str> {
    match value {
        Value::Null => output.extend_from_slice(b"null"),
        Value::Bool(true) => output.extend_from_slice(b"true"),
        Value::Bool(false) => output.extend_from_slice(b"false"),
        Value::Number(number) if number.is_i64() || number.is_u64() => {
            serde_json::to_writer(&mut *output, number)
                .map_err(|_| "cannot serialize canonical JSON integer")?;
        }
        Value::Number(_) => return Err("canonical JSON numbers must be integers"),
        Value::String(string) => {
            serde_json::to_writer(&mut *output, string)
                .map_err(|_| "cannot serialize canonical JSON string")?;
        }
        Value::Array(items) => {
            output.push(b'[');
            for (index, item) in items.iter().enumerate() {
                if index != 0 {
                    output.push(b',');
                }
                append_canonical_json(item, output)?;
            }
            output.push(b']');
        }
        Value::Object(object) => {
            let mut keys = object.keys().collect::<Vec<_>>();
            keys.sort_unstable_by(|left, right| left.as_bytes().cmp(right.as_bytes()));
            output.push(b'{');
            for (index, key) in keys.into_iter().enumerate() {
                if index != 0 {
                    output.push(b',');
                }
                serde_json::to_writer(&mut *output, key)
                    .map_err(|_| "cannot serialize canonical JSON object key")?;
                output.push(b':');
                append_canonical_json(&object[key], output)?;
            }
            output.push(b'}');
        }
    }
    Ok(())
}

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
