use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read},
    os::unix::fs::OpenOptionsExt,
    path::Path,
};

use nix::fcntl::OFlag;

use super::{
    DIRECT_SCHEMA_GENERATION_V1, DirectStoreErrorV1, DirectStoreResultV1, DirectWorkspaceLayoutV1,
    LEGACY_PODWAY_MARKER_V1, LEGACY_RUNTIME_MARKERS_V1,
};

const SQLITE_HEADER_BYTES: usize = 100;
const SQLITE_MAGIC: &[u8; 16] = b"SQLite format 3\0";
const WAL_HEADER_BYTES: usize = 32;
const WAL_FRAME_HEADER_BYTES: usize = 24;
const WAL_VERSION: u32 = 3_007_000;
/// A corrupt or continuously growing WAL cannot make classification unbounded.
const MAX_CLASSIFIED_WAL_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DirectStoreClassificationV1 {
    Missing,
    Current,
    Legacy {
        generation: Option<u32>,
        marker: Option<&'static str>,
    },
    Newer {
        generation: u32,
    },
    Maintenance,
    Unrecognized,
}

impl DirectStoreClassificationV1 {
    pub(crate) fn require_current(self) -> DirectStoreResultV1<()> {
        match self {
            Self::Current => Ok(()),
            Self::Missing => Err(DirectStoreErrorV1::NotInitialized),
            Self::Legacy { generation, marker } => {
                Err(DirectStoreErrorV1::Legacy { generation, marker })
            }
            Self::Newer { generation } => {
                Err(DirectStoreErrorV1::UnsupportedGeneration { generation })
            }
            Self::Maintenance => Err(DirectStoreErrorV1::Maintenance),
            Self::Unrecognized => Err(DirectStoreErrorV1::Unreadable),
        }
    }
}

/// Inspect headers and marker names without opening SQLite, recovering WAL or creating SHM.
pub fn classify_direct_store_v1(
    layout: &DirectWorkspaceLayoutV1,
) -> DirectStoreResultV1<DirectStoreClassificationV1> {
    classify_direct_store_with_missing_observer_v1(layout, &mut || {})
}

/// Destructive maintenance must resolve generation independently of marker diagnosis.
/// An inconclusive WAL cannot become deletion authority merely because a marker exists.
pub fn require_destructive_generation_v1(
    layout: &DirectWorkspaceLayoutV1,
) -> DirectStoreResultV1<()> {
    let directories = ClassificationDirectories::open(layout)?;
    let mut witnesses = Vec::new();
    for path in [layout.database(), layout.wal()] {
        let mut held = match open_read_nofollow(&path) {
            Ok(file) => Some(file),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error.into()),
        };
        if path == layout.database()
            && let Some(file) = held.as_mut()
        {
            let mut header = [0; SQLITE_HEADER_BYTES];
            if read_prefix(file, &mut header)? == SQLITE_HEADER_BYTES
                && &header[..16] == SQLITE_MAGIC
                && be_u32(&header[60..64]) > DIRECT_SCHEMA_GENERATION_V1
            {
                directories.revalidate(layout)?;
                if !super::maintenance::same_file(file, &path)? {
                    return Err(DirectStoreErrorV1::Maintenance);
                }
                return Err(DirectStoreErrorV1::UnsupportedGeneration {
                    generation: be_u32(&header[60..64]),
                });
            }
        }
        witnesses.push((held, path));
    }
    let result =
        classify_database_store_v1(layout).and_then(|classification| match classification {
            DirectStoreClassificationV1::Newer { generation } => {
                Err(DirectStoreErrorV1::UnsupportedGeneration { generation })
            }
            _ => Ok(()),
        });
    directories.revalidate(layout)?;
    for (held, path) in witnesses {
        let same = match held {
            Some(file) => super::maintenance::same_file(&file, &path)?,
            None => !path_exists(&path)?,
        };
        if !same {
            return Err(DirectStoreErrorV1::Maintenance);
        }
    }
    result
}

pub(super) fn classify_direct_store_with_missing_observer_v1(
    layout: &DirectWorkspaceLayoutV1,
    missing: &mut dyn FnMut(),
) -> DirectStoreResultV1<DirectStoreClassificationV1> {
    classify_at_inspection(layout, missing, || {})
}

fn classify_at_inspection(
    layout: &DirectWorkspaceLayoutV1,
    missing: &mut dyn FnMut(),
    before_inspection: impl FnOnce(),
) -> DirectStoreResultV1<DirectStoreClassificationV1> {
    let directories = ClassificationDirectories::open(layout)?;
    let mut result = if maintenance_marked(layout)? {
        Ok(match classify_database_store_v1(layout) {
            Ok(classification @ DirectStoreClassificationV1::Newer { .. }) => classification,
            _ => DirectStoreClassificationV1::Maintenance,
        })
    } else {
        before_inspection();
        classify_unmarked_store_v1(layout)
    };
    if matches!(
        result,
        Ok(DirectStoreClassificationV1::Missing | DirectStoreClassificationV1::Unrecognized)
    ) && maintenance_marked(layout)?
    {
        result = Ok(DirectStoreClassificationV1::Maintenance);
    }
    directories.revalidate(layout)?;
    if matches!(result, Ok(DirectStoreClassificationV1::Missing)) {
        missing();
    }
    result
}

pub(super) fn classify_unmarked_store_v1(
    layout: &DirectWorkspaceLayoutV1,
) -> DirectStoreResultV1<DirectStoreClassificationV1> {
    let classification = classify_database_store_v1(layout);
    if matches!(
        classification,
        Ok(DirectStoreClassificationV1::Newer { .. })
    ) {
        return classification;
    }
    for marker in LEGACY_RUNTIME_MARKERS_V1 {
        if path_exists(&layout.runtime_directory().join(marker))? {
            return Ok(DirectStoreClassificationV1::Legacy {
                generation: None,
                marker: Some(marker),
            });
        }
    }
    if path_exists(&layout.podway_directory().join(LEGACY_PODWAY_MARKER_V1))? {
        return Ok(DirectStoreClassificationV1::Legacy {
            generation: None,
            marker: Some(LEGACY_PODWAY_MARKER_V1),
        });
    }
    classification
}

fn classify_database_store_v1(
    layout: &DirectWorkspaceLayoutV1,
) -> DirectStoreResultV1<DirectStoreClassificationV1> {
    let mut file = match open_read_nofollow(&layout.database()) {
        Ok(file) => Some(file),
        Err(error) if error.kind() == io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let mut header = [0; SQLITE_HEADER_BYTES];
    let length = match file.as_mut() {
        Some(file) => read_prefix(file, &mut header)?,
        None => 0,
    };
    let main_generation = (length == SQLITE_HEADER_BYTES && &header[..16] == SQLITE_MAGIC)
        .then(|| be_u32(&header[60..64]));
    // A known unsupported main file must still refuse when its WAL is unreadable.
    if let Some(generation) = main_generation.filter(|value| *value > DIRECT_SCHEMA_GENERATION_V1) {
        return Ok(DirectStoreClassificationV1::Newer { generation });
    }
    // Even a nonzero main generation can precede a newer committed page-one WAL image.
    let Some(generation) = committed_wal_generation(&layout.wal())?.or(main_generation) else {
        return Ok(
            if file.is_none() && !path_exists(&layout.wal())? && !path_exists(&layout.shm())? {
                DirectStoreClassificationV1::Missing
            } else {
                DirectStoreClassificationV1::Unrecognized
            },
        );
    };
    if file.is_none() && generation <= DIRECT_SCHEMA_GENERATION_V1 {
        return Ok(DirectStoreClassificationV1::Unrecognized);
    }
    Ok(match generation {
        DIRECT_SCHEMA_GENERATION_V1 => DirectStoreClassificationV1::Current,
        0 => DirectStoreClassificationV1::Unrecognized,
        generation if generation < DIRECT_SCHEMA_GENERATION_V1 => {
            DirectStoreClassificationV1::Legacy {
                generation: Some(generation),
                marker: None,
            }
        }
        generation => DirectStoreClassificationV1::Newer { generation },
    })
}

pub(super) fn maintenance_marked(layout: &DirectWorkspaceLayoutV1) -> DirectStoreResultV1<bool> {
    Ok(path_exists(&layout.intent_marker())?
        || path_exists(&layout.reset_marker())?
        || path_exists(&layout.removal_marker())?)
}

pub(super) fn path_exists(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

pub(super) fn open_read_nofollow(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(OFlag::O_NOFOLLOW.bits())
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Err(io::Error::other("Store input must be a regular file"));
    }
    Ok(file)
}

fn read_prefix(reader: &mut impl Read, buffer: &mut [u8]) -> io::Result<usize> {
    let mut read = 0;
    while read < buffer.len() {
        match reader.read(&mut buffer[read..]) {
            Ok(0) => break,
            Ok(count) => read += count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(read)
}

fn be_u32(bytes: &[u8]) -> u32 {
    u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

fn committed_wal_generation(path: &Path) -> DirectStoreResultV1<Option<u32>> {
    committed_wal_generation_with_limit(path, MAX_CLASSIFIED_WAL_BYTES)
}

fn committed_wal_generation_with_limit(
    path: &Path,
    limit: u64,
) -> DirectStoreResultV1<Option<u32>> {
    let file = match open_read_nofollow(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let length = file.metadata()?.len();
    // Freeze and cap the observed length; still inspect trustworthy committed
    // prefixes before declaring a valid stream too large to resolve.
    let mut input = file.take(length.min(limit));
    let mut header = [0; WAL_HEADER_BYTES];
    if read_prefix(&mut input, &mut header)? != WAL_HEADER_BYTES {
        return Ok(None);
    }
    let magic = be_u32(&header[..4]);
    if !matches!(magic, 0x377f_0682 | 0x377f_0683) || be_u32(&header[4..8]) != WAL_VERSION {
        return Ok(None);
    }
    let page_size = be_u32(&header[8..12]) as usize;
    if !page_size.is_power_of_two() || !(512..=65_536).contains(&page_size) {
        return Ok(None);
    }
    let big_endian = magic & 1 == 1;
    let mut checksum = wal_checksum(big_endian, (0, 0), &header[..24]);
    if checksum != (be_u32(&header[24..28]), be_u32(&header[28..32])) {
        return Ok(None);
    }
    let mut frame = vec![0; WAL_FRAME_HEADER_BYTES + page_size];
    let mut pending_generation = None;
    let mut committed_generation = None;
    loop {
        if read_prefix(&mut input, &mut frame)? != frame.len() {
            return if length > limit {
                Err(DirectStoreErrorV1::Unreadable)
            } else {
                Ok(committed_generation)
            };
        }
        if frame[8..16] != header[16..24] || be_u32(&frame[..4]) == 0 {
            return Ok(committed_generation);
        }
        checksum = wal_checksum(big_endian, checksum, &frame[..8]);
        checksum = wal_checksum(big_endian, checksum, &frame[WAL_FRAME_HEADER_BYTES..]);
        if checksum != (be_u32(&frame[16..20]), be_u32(&frame[20..24])) {
            return Ok(committed_generation);
        }
        if be_u32(&frame[..4]) == 1 {
            let page = &frame[WAL_FRAME_HEADER_BYTES..];
            pending_generation = Some((&page[..16] == SQLITE_MAGIC).then(|| be_u32(&page[60..64])));
        }
        if be_u32(&frame[4..8]) != 0
            && let Some(generation) = pending_generation
        {
            let generation = generation.ok_or(DirectStoreErrorV1::Unreadable)?;
            // A validated committed unsupported image is sufficient to refuse;
            // neither a huge tail nor a later downgrade grants an older writer authority.
            if generation > DIRECT_SCHEMA_GENERATION_V1 {
                return Ok(Some(generation));
            }
            committed_generation = Some(generation);
        }
    }
}

/// Parent components are also no-follow inputs. A raced directory replacement is
/// retried by the caller rather than allowing a path lookup to authorize a new inode.
pub(super) struct ClassificationDirectories {
    podway: Option<File>,
    runtime: Option<File>,
}

impl ClassificationDirectories {
    pub(super) fn open(layout: &DirectWorkspaceLayoutV1) -> DirectStoreResultV1<Self> {
        let podway = optional_directory(&layout.podway_directory())?;
        let runtime = if podway.is_some() {
            optional_directory(&layout.runtime_directory())?
        } else {
            None
        };
        Ok(Self { podway, runtime })
    }

    pub(super) fn revalidate(&self, layout: &DirectWorkspaceLayoutV1) -> DirectStoreResultV1<()> {
        for (held, path) in [
            (&self.podway, layout.podway_directory()),
            (&self.runtime, layout.runtime_directory()),
        ] {
            let same = match held {
                Some(file) => super::maintenance::same_file(file, &path)?,
                None => !path_exists(&path)?,
            };
            if !same {
                return Err(DirectStoreErrorV1::Maintenance);
            }
        }
        Ok(())
    }
}

fn optional_directory(path: &Path) -> io::Result<Option<File>> {
    match super::maintenance::open_directory(path) {
        Ok(file) => Ok(Some(file)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn wal_checksum(big_endian: bool, seed: (u32, u32), bytes: &[u8]) -> (u32, u32) {
    let (mut first, mut second) = seed;
    for pair in bytes.chunks_exact(8) {
        let word = |bytes: &[u8]| {
            let bytes = [bytes[0], bytes[1], bytes[2], bytes[3]];
            if big_endian {
                u32::from_be_bytes(bytes)
            } else {
                u32::from_le_bytes(bytes)
            }
        };
        first = first.wrapping_add(word(&pair[..4])).wrapping_add(second);
        second = second.wrapping_add(word(&pair[4..])).wrapping_add(first);
    }
    (first, second)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publication_during_missing_inspection_is_maintenance_and_unmarked_failures_stay_closed() {
        use super::super::{
            DirectMaintenanceMarkerKindV1, DirectMaintenanceMarkerV1, ExclusiveMaintenanceLeaseV1,
        };
        use crate::CanonicalExecutionJsonV1;
        use podway_core::canonicalize_json_v1;
        use serde_json::json;
        let root = tempfile::TempDir::new().unwrap();
        let layout = DirectWorkspaceLayoutV1::new(
            crate::ValidatedWorkspaceRootV1::from_path(root.path()).unwrap(),
        )
        .unwrap();
        let lease = ExclusiveMaintenanceLeaseV1::acquire(
            &layout,
            std::time::Instant::now() + std::time::Duration::from_secs(10),
            true,
        )
        .unwrap();
        let digest = format!("sha256:{}", "a".repeat(64));
        let value = json!({"schema":"podway.workspace-maintenance-intent-marker/v1","command":"workspace.init",
            "operation_id":"00000000-0000-4000-8000-000000000003","response_request_id":"00000000-0000-4000-8000-000000000002",
            "workspace_uuid":null,"idempotency_key":"publish-race","request_digest":digest,"contract_manifest_digest":digest,
            "created_at":"1970-01-01T00:00:00.010Z","root_identity":{"root_path_bytes_base64url":"L3RtcA",
                "common_dir_identity":digest,"worktree_admin_identity":digest}});
        let marker = DirectMaintenanceMarkerV1::new(
            DirectMaintenanceMarkerKindV1::Intent,
            CanonicalExecutionJsonV1::new(canonicalize_json_v1(&value).unwrap()).unwrap(),
        )
        .unwrap();
        let mut observed = 0;
        let result = classify_at_inspection(&layout, &mut || observed += 1, || {
            lease.write_marker(&layout, &marker, None).unwrap()
        })
        .unwrap();
        assert_eq!(result, DirectStoreClassificationV1::Maintenance);
        assert_eq!(observed, 0);
        lease.remove_marker(&layout, &marker).unwrap();
        let result =
            classify_direct_store_with_missing_observer_v1(&layout, &mut || observed += 1).unwrap();
        assert_eq!(result, DirectStoreClassificationV1::Missing);
        assert_eq!(observed, 1);
        fs::write(layout.database(), b"corrupt unmarked main").unwrap();
        assert_eq!(
            classify_direct_store_with_missing_observer_v1(&layout, &mut || observed += 1).unwrap(),
            DirectStoreClassificationV1::Unrecognized
        );
        assert_eq!(
            fs::read(layout.database()).unwrap(),
            b"corrupt unmarked main"
        );
        assert!(!layout.wal().exists());
        assert!(!layout.shm().exists());
        assert_eq!(
            classify_at_inspection(&layout, &mut || observed += 1, || lease
                .write_marker(&layout, &marker, None)
                .unwrap())
            .unwrap(),
            DirectStoreClassificationV1::Maintenance
        );
        assert_eq!(observed, 1);
        lease.remove_marker(&layout, &marker).unwrap();
        drop(lease);
        let other = tempfile::TempDir::new().unwrap();
        fs::create_dir(other.path().join(".podway")).unwrap();
        std::os::unix::fs::symlink(root.path(), other.path().join(".podway/runtime")).unwrap();
        let unsafe_layout = DirectWorkspaceLayoutV1::new(
            crate::ValidatedWorkspaceRootV1::from_path(other.path()).unwrap(),
        )
        .unwrap();
        assert!(
            classify_direct_store_with_missing_observer_v1(&unsafe_layout, &mut || observed += 1)
                .is_err()
        );
        assert_eq!(observed, 1);
    }

    fn wal(big_endian: bool, frames: &[(u32, u32, Option<u32>)]) -> Vec<u8> {
        let mut bytes = vec![0; WAL_HEADER_BYTES];
        let magic = if big_endian {
            0x377f0683_u32
        } else {
            0x377f0682_u32
        };
        bytes[..4].copy_from_slice(&magic.to_be_bytes());
        bytes[4..8].copy_from_slice(&WAL_VERSION.to_be_bytes());
        bytes[8..12].copy_from_slice(&512_u32.to_be_bytes());
        bytes[16..24].copy_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8]);
        let mut sum = wal_checksum(big_endian, (0, 0), &bytes[..24]);
        bytes[24..28].copy_from_slice(&sum.0.to_be_bytes());
        bytes[28..32].copy_from_slice(&sum.1.to_be_bytes());
        for (page, commit, generation) in frames {
            let mut frame = vec![0; WAL_FRAME_HEADER_BYTES + 512];
            frame[..4].copy_from_slice(&page.to_be_bytes());
            frame[4..8].copy_from_slice(&commit.to_be_bytes());
            frame[8..16].copy_from_slice(&bytes[16..24]);
            if let Some(generation) = generation {
                frame[24..40].copy_from_slice(SQLITE_MAGIC);
                frame[84..88].copy_from_slice(&generation.to_be_bytes());
            }
            sum = wal_checksum(big_endian, sum, &frame[..8]);
            sum = wal_checksum(big_endian, sum, &frame[24..]);
            frame[16..20].copy_from_slice(&sum.0.to_be_bytes());
            frame[20..24].copy_from_slice(&sum.1.to_be_bytes());
            bytes.extend(frame);
        }
        bytes
    }

    #[test]
    fn checksum_endianness_and_committed_page_one_are_not_confused_with_a_tail() {
        assert_eq!(
            wal_checksum(false, (0, 0), &[0, 1, 2, 3, 4, 5, 6, 7]),
            (50_462_976, 168_297_988)
        );
        assert_eq!(
            wal_checksum(true, (0, 0), &[0, 1, 2, 3, 4, 5, 6, 7]),
            (66_051, 67_504_138)
        );
        let root = tempfile::TempDir::new().unwrap();
        let path = root.path().join("test-wal");
        for big in [false, true] {
            fs::write(
                &path,
                wal(big, &[(1, 0, Some(14)), (2, 2, None), (1, 0, Some(15))]),
            )
            .unwrap();
            assert_eq!(committed_wal_generation(&path).unwrap(), Some(14));
            let mut truncated = wal(big, &[(1, 1, Some(14)), (1, 1, Some(15))]);
            truncated.truncate(truncated.len() - 1);
            fs::write(&path, truncated).unwrap();
            assert_eq!(committed_wal_generation(&path).unwrap(), Some(14));
            let mut corrupt = wal(big, &[(1, 1, Some(14)), (1, 1, Some(15))]);
            *corrupt.last_mut().unwrap() ^= 1;
            fs::write(&path, corrupt).unwrap();
            assert_eq!(committed_wal_generation(&path).unwrap(), Some(14));
            fs::write(&path, wal(big, &[(1, 0, None)])).unwrap();
            assert_eq!(committed_wal_generation(&path).unwrap(), None);
            fs::write(&path, wal(big, &[(1, 1, None)])).unwrap();
            assert!(matches!(
                committed_wal_generation(&path),
                Err(DirectStoreErrorV1::Unreadable)
            ));
        }
    }

    const NEWER_GENERATION: u32 = DIRECT_SCHEMA_GENERATION_V1 + 1;

    #[test]
    fn unresolved_valid_wal_stream_is_bounded_and_known_main_newer_ignores_unsafe_wal() {
        let root = tempfile::TempDir::new().unwrap();
        let layout = DirectWorkspaceLayoutV1::new(
            crate::ValidatedWorkspaceRootV1::from_path(root.path()).unwrap(),
        )
        .unwrap();
        fs::create_dir_all(layout.runtime_directory()).unwrap();
        let first_frame = (WAL_HEADER_BYTES + WAL_FRAME_HEADER_BYTES + 512) as u64;
        fs::write(
            layout.wal(),
            wal(
                false,
                &[
                    (1, 1, Some(DIRECT_SCHEMA_GENERATION_V1)),
                    (1, 1, Some(NEWER_GENERATION)),
                ],
            ),
        )
        .unwrap();
        assert!(matches!(
            committed_wal_generation_with_limit(&layout.wal(), first_frame),
            Err(DirectStoreErrorV1::Unreadable)
        ));
        assert_eq!(
            committed_wal_generation(&layout.wal()).unwrap(),
            Some(NEWER_GENERATION)
        );
        fs::remove_file(layout.wal()).unwrap();
        std::os::unix::fs::symlink("missing-wal", layout.wal()).unwrap();
        let mut main = [0; SQLITE_HEADER_BYTES];
        main[..16].copy_from_slice(SQLITE_MAGIC);
        main[60..64].copy_from_slice(&NEWER_GENERATION.to_be_bytes());
        fs::write(layout.database(), main).unwrap();
        assert_eq!(
            classify_direct_store_v1(&layout).unwrap(),
            DirectStoreClassificationV1::Newer {
                generation: NEWER_GENERATION
            }
        );
        assert!(matches!(
            require_destructive_generation_v1(&layout),
            Err(DirectStoreErrorV1::UnsupportedGeneration {
                generation: NEWER_GENERATION
            })
        ));
        assert_eq!(fs::read(layout.database()).unwrap(), main);
        assert!(
            fs::symlink_metadata(layout.wal())
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(!layout.shm().exists());
    }

    #[test]
    fn committed_wal_overrides_a_nonzero_main_header_and_scan_size_is_bounded() {
        let root = tempfile::TempDir::new().unwrap();
        let layout = DirectWorkspaceLayoutV1::new(
            crate::ValidatedWorkspaceRootV1::from_path(root.path()).unwrap(),
        )
        .unwrap();
        fs::create_dir_all(layout.runtime_directory()).unwrap();
        let mut main = vec![0; SQLITE_HEADER_BYTES];
        main[..16].copy_from_slice(SQLITE_MAGIC);
        main[60..64].copy_from_slice(&13_u32.to_be_bytes());
        fs::write(layout.database(), main).unwrap();
        fs::write(
            layout.wal(),
            wal(false, &[(1, 1, Some(DIRECT_SCHEMA_GENERATION_V1))]),
        )
        .unwrap();
        assert_eq!(
            classify_direct_store_v1(&layout).unwrap(),
            DirectStoreClassificationV1::Current
        );
        fs::write(layout.wal(), wal(true, &[(1, 1, Some(NEWER_GENERATION))])).unwrap();
        assert_eq!(
            classify_direct_store_v1(&layout).unwrap(),
            DirectStoreClassificationV1::Newer {
                generation: NEWER_GENERATION
            }
        );
        let file = OpenOptions::new().write(true).open(layout.wal()).unwrap();
        file.set_len(MAX_CLASSIFIED_WAL_BYTES + 1).unwrap();
        assert_eq!(
            classify_direct_store_v1(&layout).unwrap(),
            DirectStoreClassificationV1::Newer {
                generation: NEWER_GENERATION
            }
        );
        assert!(!layout.shm().exists());
    }
    #[test]
    fn committed_newer_wal_refuses_markers_overbound_tail_and_missing_main() {
        for main_present in [false, true] {
            for marker in [
                None,
                Some("reset.marker"),
                Some("reset-all.marker"),
                Some("mode-switch.json"),
            ] {
                let root = tempfile::TempDir::new().unwrap();
                let layout = DirectWorkspaceLayoutV1::new(
                    crate::ValidatedWorkspaceRootV1::from_path(root.path()).unwrap(),
                )
                .unwrap();
                fs::create_dir_all(layout.runtime_directory()).unwrap();
                if main_present {
                    let mut main = [0; SQLITE_HEADER_BYTES];
                    main[..16].copy_from_slice(SQLITE_MAGIC);
                    main[60..64].copy_from_slice(&DIRECT_SCHEMA_GENERATION_V1.to_be_bytes());
                    fs::write(layout.database(), main).unwrap();
                }
                fs::write(layout.wal(), wal(false, &[(1, 1, Some(NEWER_GENERATION))])).unwrap();
                OpenOptions::new()
                    .write(true)
                    .open(layout.wal())
                    .unwrap()
                    .set_len(MAX_CLASSIFIED_WAL_BYTES + 1)
                    .unwrap();
                if let Some(marker) = marker {
                    let directory = if marker == "mode-switch.json" {
                        layout.podway_directory()
                    } else {
                        layout.runtime_directory()
                    };
                    fs::write(directory.join(marker), b"foreign marker").unwrap();
                }
                assert_eq!(
                    classify_direct_store_v1(&layout).unwrap(),
                    DirectStoreClassificationV1::Newer {
                        generation: NEWER_GENERATION
                    },
                    "main={main_present}, marker={marker:?}"
                );
                assert!(!layout.shm().exists());
            }
        }
    }

    #[test]
    fn marked_wal_scan_failures_preserve_recovery_and_known_newer_refusal() {
        use std::os::unix::fs::MetadataExt;
        for generation in [DIRECT_SCHEMA_GENERATION_V1, NEWER_GENERATION] {
            for oversized in [false, true] {
                for marker in [
                    None,
                    Some("maintenance-intent.json"),
                    Some("reset-all.marker"),
                    Some("removal.marker"),
                    Some(LEGACY_RUNTIME_MARKERS_V1[0]),
                    Some(LEGACY_PODWAY_MARKER_V1),
                ] {
                    let root = tempfile::TempDir::new().unwrap();
                    let layout = DirectWorkspaceLayoutV1::new(
                        crate::ValidatedWorkspaceRootV1::from_path(root.path()).unwrap(),
                    )
                    .unwrap();
                    fs::create_dir_all(layout.runtime_directory()).unwrap();
                    let mut main = [0; SQLITE_HEADER_BYTES];
                    main[..16].copy_from_slice(SQLITE_MAGIC);
                    main[60..64].copy_from_slice(&generation.to_be_bytes());
                    fs::write(layout.database(), main).unwrap();
                    let bytes = wal(false, &[(1, 1, None)]);
                    fs::write(layout.wal(), &bytes).unwrap();
                    if oversized {
                        OpenOptions::new()
                            .write(true)
                            .open(layout.wal())
                            .unwrap()
                            .set_len(MAX_CLASSIFIED_WAL_BYTES + 1)
                            .unwrap();
                    }
                    if let Some(marker) = marker {
                        let directory = if marker == LEGACY_PODWAY_MARKER_V1 {
                            layout.podway_directory()
                        } else {
                            layout.runtime_directory()
                        };
                        fs::write(directory.join(marker), b"retained marker").unwrap();
                    }
                    let before = fs::metadata(layout.wal()).unwrap();
                    let result = classify_direct_store_v1(&layout);
                    let admission = require_destructive_generation_v1(&layout);
                    if generation > DIRECT_SCHEMA_GENERATION_V1 {
                        assert!(matches!(
                            admission,
                            Err(DirectStoreErrorV1::UnsupportedGeneration {
                                generation: NEWER_GENERATION
                            })
                        ));
                    } else {
                        assert!(matches!(admission, Err(DirectStoreErrorV1::Unreadable)));
                    }
                    if generation > DIRECT_SCHEMA_GENERATION_V1 {
                        assert_eq!(
                            result.unwrap(),
                            DirectStoreClassificationV1::Newer { generation }
                        );
                    } else if let Some(marker) = marker {
                        let expected = if marker == LEGACY_PODWAY_MARKER_V1
                            || LEGACY_RUNTIME_MARKERS_V1.contains(&marker)
                        {
                            DirectStoreClassificationV1::Legacy {
                                generation: None,
                                marker: Some(marker),
                            }
                        } else {
                            DirectStoreClassificationV1::Maintenance
                        };
                        assert_eq!(result.unwrap(), expected);
                    } else {
                        assert!(matches!(result, Err(DirectStoreErrorV1::Unreadable)));
                    }
                    assert_eq!(fs::read(layout.database()).unwrap(), main);
                    let after = fs::metadata(layout.wal()).unwrap();
                    assert_eq!(
                        (after.dev(), after.ino(), after.len()),
                        (before.dev(), before.ino(), before.len())
                    );
                    let mut prefix = vec![0; bytes.len()];
                    open_read_nofollow(&layout.wal())
                        .unwrap()
                        .read_exact(&mut prefix)
                        .unwrap();
                    assert_eq!(prefix, bytes);
                    assert!(!layout.shm().exists());
                }
            }
        }
    }
}
