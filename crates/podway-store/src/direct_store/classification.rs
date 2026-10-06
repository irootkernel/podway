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
    let directories = ClassificationDirectories::open(layout)?;
    let result = classify_marked_store_v1(layout);
    directories.revalidate(layout)?;
    result
}

fn classify_marked_store_v1(
    layout: &DirectWorkspaceLayoutV1,
) -> DirectStoreResultV1<DirectStoreClassificationV1> {
    if maintenance_marked(layout)? {
        return Ok(DirectStoreClassificationV1::Maintenance);
    }
    classify_unmarked_store_v1(layout)
}

pub(super) fn classify_unmarked_store_v1(
    layout: &DirectWorkspaceLayoutV1,
) -> DirectStoreResultV1<DirectStoreClassificationV1> {
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
    let mut file = match open_read_nofollow(&layout.database()) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(
                if path_exists(&layout.wal())? || path_exists(&layout.shm())? {
                    DirectStoreClassificationV1::Unrecognized
                } else {
                    DirectStoreClassificationV1::Missing
                },
            );
        }
        Err(error) => return Err(error.into()),
    };
    let mut header = [0; SQLITE_HEADER_BYTES];
    let length = read_prefix(&mut file, &mut header)?;
    let main_generation = (length == SQLITE_HEADER_BYTES && &header[..16] == SQLITE_MAGIC)
        .then(|| be_u32(&header[60..64]));
    // Even a nonzero main generation can precede a newer committed page-one WAL image.
    let Some(generation) = committed_wal_generation(&layout.wal())?.or(main_generation) else {
        return Ok(DirectStoreClassificationV1::Unrecognized);
    };
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
    let file = match open_read_nofollow(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let length = file.metadata()?.len();
    if length > MAX_CLASSIFIED_WAL_BYTES {
        return Err(DirectStoreErrorV1::Unreadable);
    }
    // Freeze the observed length; an appending writer cannot extend this read's budget.
    let mut input = file.take(length);
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
    while read_prefix(&mut input, &mut frame)? == frame.len() {
        if frame[8..16] != header[16..24] || be_u32(&frame[..4]) == 0 {
            break;
        }
        checksum = wal_checksum(big_endian, checksum, &frame[..8]);
        checksum = wal_checksum(big_endian, checksum, &frame[WAL_FRAME_HEADER_BYTES..]);
        if checksum != (be_u32(&frame[16..20]), be_u32(&frame[20..24])) {
            break;
        }
        if be_u32(&frame[..4]) == 1 {
            let page = &frame[WAL_FRAME_HEADER_BYTES..];
            pending_generation = Some((&page[..16] == SQLITE_MAGIC).then(|| be_u32(&page[60..64])));
        }
        if be_u32(&frame[4..8]) != 0
            && let Some(generation) = pending_generation
        {
            committed_generation = Some(generation.ok_or(DirectStoreErrorV1::Unreadable)?);
        }
    }
    Ok(committed_generation)
}

/// Parent components are also no-follow inputs. A raced directory replacement is
/// retried by the caller rather than allowing a path lookup to authorize a new inode.
struct ClassificationDirectories {
    podway: Option<File>,
    runtime: Option<File>,
}

impl ClassificationDirectories {
    fn open(layout: &DirectWorkspaceLayoutV1) -> DirectStoreResultV1<Self> {
        let podway = optional_directory(&layout.podway_directory())?;
        let runtime = if podway.is_some() {
            optional_directory(&layout.runtime_directory())?
        } else {
            None
        };
        Ok(Self { podway, runtime })
    }

    fn revalidate(&self, layout: &DirectWorkspaceLayoutV1) -> DirectStoreResultV1<()> {
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
        fs::write(layout.wal(), wal(false, &[(1, 1, Some(14))])).unwrap();
        assert_eq!(
            classify_direct_store_v1(&layout).unwrap(),
            DirectStoreClassificationV1::Current
        );
        fs::write(layout.wal(), wal(true, &[(1, 1, Some(15))])).unwrap();
        assert_eq!(
            classify_direct_store_v1(&layout).unwrap(),
            DirectStoreClassificationV1::Newer { generation: 15 }
        );
        let file = OpenOptions::new().write(true).open(layout.wal()).unwrap();
        file.set_len(MAX_CLASSIFIED_WAL_BYTES + 1).unwrap();
        assert!(matches!(
            classify_direct_store_v1(&layout),
            Err(DirectStoreErrorV1::Unreadable)
        ));
        assert!(!layout.shm().exists());
    }
}
