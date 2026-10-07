//! Fixture-only support for real CLI qualification; never writes domain state.

use std::{
    fs::{self, OpenOptions},
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use podway_core::{Sha256Digest, WorkspaceId};
use podway_engine::workspace::DirectWorkspaceResolverV1;
use podway_git::{
    DiagnosticPathDisplayV1, LosslessPathV1, NativeGitResolverV1, WORKTREE_SELECTOR_VERSION_V1,
    WorktreeSelectorV1,
};
use podway_protocol::validate_daemon_free_document_v1;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields, tag = "mode", rename_all = "kebab-case")]
enum Input {
    Reader {
        evidence_root: PathBuf,
        worktree: PathBuf,
        workspace_uuid: String,
        manifest: String,
        ready: PathBuf,
    },
    ValidateOutput {
        evidence_root: PathBuf,
        index: PathBuf,
    },
}

fn owned_path(root: &Path, path: &Path) {
    assert!(root.is_absolute() && path.is_absolute());
    assert_eq!(root.canonicalize().unwrap(), root);
    assert!(path.starts_with(root) && path != root);
    let mut cursor = path;
    while cursor != root {
        if let Ok(metadata) = fs::symlink_metadata(cursor) {
            assert!(!metadata.file_type().is_symlink());
        }
        cursor = cursor.parent().unwrap();
    }
}

fn bounded_json(path: &Path, limit: u64) -> serde_json::Value {
    assert!(fs::symlink_metadata(path).unwrap().is_file());
    let mut bytes = Vec::new();
    fs::File::open(path)
        .unwrap()
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .unwrap();
    assert!(bytes.len() as u64 <= limit);
    serde_json::from_slice(&bytes).unwrap()
}

#[test]
#[ignore = "fixture support invoked by the real CLI qualification driver"]
fn qualified_reader_and_output_validator() {
    let Some(input_path) = std::env::var_os("PODWAY_CLI_QUALIFICATION_INPUT") else {
        return;
    };
    let input_path = PathBuf::from(input_path);
    let input: Input = serde_json::from_value(bounded_json(&input_path, 16_384)).unwrap();
    match input {
        Input::Reader {
            evidence_root,
            worktree,
            workspace_uuid,
            manifest,
            ready,
        } => {
            owned_path(&evidence_root, &worktree);
            owned_path(&evidence_root, &ready);
            assert!(worktree.join(".git").is_dir());
            assert_eq!(worktree.canonicalize().unwrap(), worktree);
            let selector = WorktreeSelectorV1::new(
                WORKTREE_SELECTOR_VERSION_V1,
                None,
                LosslessPathV1::from_raw_bytes(
                    worktree.as_os_str().as_encoded_bytes(),
                    DiagnosticPathDisplayV1::new(worktree.to_str().unwrap()).unwrap(),
                )
                .unwrap(),
            )
            .unwrap();
            let expected = WorkspaceId::new(workspace_uuid).unwrap();
            let resolver = DirectWorkspaceResolverV1::new(
                NativeGitResolverV1::new(),
                Sha256Digest::new(manifest).unwrap(),
            );
            let deadline = Instant::now() + Duration::from_secs(30);
            let (resolved, store) = resolver
                .resolve_existing(selector, Some(&expected), deadline)
                .unwrap();
            // A cold read owns an exclusive native lease and cannot pin the
            // shared WAL index while real CLI writers continue.
            assert!(store.layout().wal().is_file());
            assert!(fs::metadata(store.layout().shm()).unwrap().len() >= 32_768);
            store
                .read::<()>(deadline, |snapshot| {
                    resolver.revalidate(&resolved, snapshot.binding()).unwrap();
                    assert_eq!(snapshot.binding().identity().workspace_uuid(), &expected);
                    let sequence = snapshot.committed_sequence()?;
                    let pending = ready.with_extension("pending");
                    owned_path(&evidence_root, &pending);
                    let mut signal = OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .open(&pending)
                        .unwrap();
                    write!(signal, "{sequence}").unwrap();
                    signal.sync_all().unwrap();
                    assert!(!ready.exists());
                    fs::rename(&pending, &ready).unwrap();
                    let expires = Instant::now() + Duration::from_secs(180);
                    while Instant::now() < expires {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    panic!("qualification parent did not reap the owned pinned reader");
                })
                .unwrap();
        }
        Input::ValidateOutput {
            evidence_root,
            index,
        } => {
            owned_path(&evidence_root, &index);
            assert!(fs::symlink_metadata(&index).unwrap().is_file());
            let mut lines = BufReader::new(fs::File::open(index).unwrap());
            let mut count = 0;
            loop {
                let mut line = Vec::new();
                let read = (&mut lines)
                    .take(16_385)
                    .read_until(b'\n', &mut line)
                    .unwrap();
                if read == 0 {
                    break;
                }
                assert!(line.len() <= 16_384 && line.last() == Some(&b'\n'));
                let record: serde_json::Value = serde_json::from_slice(&line).unwrap();
                let output = evidence_root.join(record["output"].as_str().unwrap());
                owned_path(&evidence_root, &output);
                let value = bounded_json(&output, 2_097_152);
                assert!(validate_daemon_free_document_v1("podway.output/v4", &value));
                assert_eq!(value["command"], record["command"]);
                count += 1;
                assert!(count <= 20_000);
            }
            assert!(count > 0);
            println!("validated_outputs={count}");
        }
    }
}
