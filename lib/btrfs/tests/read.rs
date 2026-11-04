// Reading the bytes back
//
// Files staged on the host go through `mkfs.btrfs -r`, compared byte for byte
// at sizes crossing the format's boundaries:
//
//   0            no extent at all
//   1..~2000     inside a leaf as an inline extent
//   4096         exactly one sector
//   4097         a sector and a byte, where a forgotten remainder shows
//   1 MiB        several sectors, consulting the checksum tree many times
//
// Plus a sparse file: a hole has no record under `NO_HOLES`, read as zeros.

mod common;

use btrfs::error::BtrfsError;
use btrfs::file::*;
use btrfs::fs_tree::*;
use btrfs::{Key, Volume};
use common::Image;
use fshost::{BlockHost, HostError};
use std::io::{Seek, SeekFrom, Write};
use std::path::PathBuf;

struct FileHost {
    data: Vec<u8>,
}

impl BlockHost for FileHost {
    fn read_sectors(&mut self, lba: u64, count: u32, buf: &mut [u8]) -> Result<(), HostError> {
        let start = (lba as usize) * 512;
        let end = start + (count as usize) * 512;
        let src = self.data.get(start..end).ok_or(HostError::Io)?;
        let n = buf.len().min(src.len());
        buf[..n].copy_from_slice(&src[..n]);
        Ok(())
    }
    fn write_sectors(&mut self, _: u64, _: u32, _: &[u8]) -> Result<(), HostError> {
        Err(HostError::Unsupported)
    }
    fn write_sectors_sync(&mut self, _: u64, _: u32, _: &[u8]) -> Result<(), HostError> {
        Err(HostError::Unsupported)
    }
    fn write_sectors_barrier(&mut self, _: u64, _: u32, _: &[u8]) -> Result<(), HostError> {
        Err(HostError::Unsupported)
    }
    fn flush(&mut self) -> Result<(), HostError> {
        Ok(())
    }
    fn total_sectors(&self) -> u64 {
        (self.data.len() / 512) as u64
    }
}

/// Bodies that are not one byte, which would hide a small offset error
fn body(seed: u64, len: usize) -> Vec<u8> {
    let mut v = Vec::with_capacity(len);
    let mut x = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
    for _ in 0..len {
        x = x.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        v.push((x >> 33) as u8);
    }
    v
}

struct Staged {
    dir: PathBuf,
    files: Vec<(String, Vec<u8>)>,
    /// A file with a hole: (name, total size, where the end data starts, the
    /// data)
    sparse: (String, u64, u64, Vec<u8>),
}

impl Drop for Staged {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn stage(tag: &str) -> Staged {
    let mut dir = std::env::temp_dir();
    dir.push(format!("btrfs-read-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let mut files = Vec::new();
    for (name, len) in [
        ("empty", 0usize),
        ("tiny", 1),
        ("inline", 1000),
        ("inline-edge", 2000),
        ("one-sector", 4096),
        ("sector-plus-one", 4097),
        ("two-sectors", 8192),
        ("big", 1024 * 1024),
        ("odd", 300_001),
    ] {
        let b = body(len as u64 + 7, len);
        std::fs::File::create(dir.join(name)).unwrap().write_all(&b).unwrap();
        files.push((name.to_string(), b));
    }

    // A hole: seek past the start and write at the end, leaving no middle
    // extent record under NO_HOLES
    let tail = body(99, 4096);
    let tail_at = 1024u64 * 1024;
    let total = tail_at + tail.len() as u64;
    {
        let mut f = std::fs::File::create(dir.join("sparse")).unwrap();
        f.seek(SeekFrom::Start(tail_at)).unwrap();
        f.write_all(&tail).unwrap();
    }

    Staged { dir, files, sparse: ("sparse".into(), total, tail_at, tail) }
}

struct Fixture {
    _img: Image,
    vol: Volume<FileHost>,
    tree: u64,
    dir: u64,
    staged: Staged,
}

fn fixture(tag: &str) -> Option<Fixture> {
    let staged = stage(tag);
    let img = Image::new(tag, Some(&staged.dir))?;
    let host = FileHost { data: std::fs::read(&img.path).expect("cannot read image") };
    let mut vol = Volume::open(host, 0).expect("a fresh volume must open");
    vol.load_chunk_tree().expect("the chunk tree must load");
    let root = vol.fs_root().expect("the fs tree must be findable");
    Some(Fixture { _img: img, vol, tree: root.bytenr, dir: root.root_dirid, staged })
}

macro_rules! fx {
    ($tag:expr) => {
        match fixture($tag) {
            Some(f) => f,
            None => {
                eprintln!("btrfs-progs not available - skipping");
                return;
            }
        }
    };
}

impl Fixture {
    fn ino_of(&mut self, name: &str) -> u64 {
        self.vol
            .lookup(self.tree, self.dir, name.as_bytes())
            .unwrap_or_else(|e| panic!("{name}: {e:?}"))
            .location
            .objectid
    }

    /// The whole file in a loop until zero, as a caller reads
    fn read_all(&mut self, ino: u64, size: usize) -> Vec<u8> {
        let mut out = vec![0u8; size];
        let mut done = 0usize;
        while done < size {
            let n = self
                .vol
                .read_file(self.tree, ino, done as u64, &mut out[done..])
                .unwrap_or_else(|e| panic!("read at {done}: {e:?}"));
            if n == 0 {
                break;
            }
            done += n;
        }
        assert_eq!(done, size, "the file ended early");
        out
    }
}

/// **The one that matters.** Every staged file comes back byte for byte.
#[test]
fn every_file_reads_back_exactly() {
    let mut f = fx!("exact");
    let names: Vec<(String, Vec<u8>)> = f.staged.files.clone();

    for (name, expected) in &names {
        let ino = f.ino_of(name);
        let got = f.read_all(ino, expected.len());
        assert_eq!(
            got.len(),
            expected.len(),
            "{name}: read {} bytes, staged {}",
            got.len(),
            expected.len()
        );
        if got != *expected {
            let at = got
                .iter()
                .zip(expected.iter())
                .position(|(a, b)| a != b)
                .unwrap_or(0);
            panic!(
                "{name}: first difference at byte {at} - got {:02x}, staged {:02x}",
                got[at], expected[at]
            );
        }
    }
}

/// A hole has no record under `NO_HOLES`, so it reads as zeros
#[test]
fn a_hole_reads_as_zeros() {
    let mut f = fx!("sparse");
    let (name, total, tail_at, tail) = f.staged.sparse.clone();
    let ino = f.ino_of(&name);

    let got = f.read_all(ino, total as usize);
    assert_eq!(got.len(), total as usize);

    assert!(
        got[..tail_at as usize].iter().all(|&b| b == 0),
        "the hole did not read as zeros"
    );
    assert_eq!(
        &got[tail_at as usize..],
        &tail[..],
        "the data after the hole is wrong"
    );
}

/// Reading part-way in, off sector boundaries, matches the whole. An ignored
/// extent `offset` would show here
#[test]
fn a_partial_read_matches_the_whole() {
    let mut f = fx!("partial");
    let expected = f
        .staged
        .files
        .iter()
        .find(|(n, _)| n == "big")
        .map(|(_, b)| b.clone())
        .unwrap();
    let ino = f.ino_of("big");

    for (at, len) in [
        (0usize, 1usize),
        (1, 1),
        (4095, 2),
        (4096, 4096),
        (5000, 7000),
        (expected.len() - 1, 1),
        (expected.len() - 4097, 4097),
        (123_456, 65_536),
    ] {
        let mut buf = vec![0u8; len];
        let n = f
            .vol
            .read_file(f.tree, ino, at as u64, &mut buf)
            .unwrap_or_else(|e| panic!("at {at} len {len}: {e:?}"));
        assert_eq!(n, len, "short read at {at}");
        assert_eq!(
            &buf[..],
            &expected[at..at + len],
            "bytes differ at offset {at} length {len}"
        );
    }
}

/// Past the end is zero bytes
#[test]
fn reading_past_the_end_returns_nothing() {
    let mut f = fx!("past");
    let ino = f.ino_of("one-sector");

    let mut buf = [0u8; 256];
    assert_eq!(f.vol.read_file(f.tree, ino, 4096, &mut buf).unwrap(), 0);
    assert_eq!(f.vol.read_file(f.tree, ino, 1 << 40, &mut buf).unwrap(), 0);

    // A read straddling the end is short. The buffer is larger than what is
    // left
    let n = f.vol.read_file(f.tree, ino, 4000, &mut buf).unwrap();
    assert_eq!(n, 96, "a read crossing EOF must stop there");
}

/// An empty file has no extent, and reading it is not an error
#[test]
fn an_empty_file_reads_as_nothing() {
    let mut f = fx!("empty");
    let ino = f.ino_of("empty");
    let mut buf = [0u8; 16];
    assert_eq!(f.vol.read_file(f.tree, ino, 0, &mut buf).unwrap(), 0);
}

/// A small file lives in its leaf. Its inode reports the true size
#[test]
fn a_small_file_is_inline() {
    let mut f = fx!("inline");
    let ino = f.ino_of("inline");

    // Its extent record says so
    let key = Key { objectid: ino, kind: EXTENT_DATA_KEY, offset: 0 };
    let item = f.vol.find_item(f.tree, &key).expect("an extent record");
    let ext = parse_extent(&key, &item, f.vol.geometry.sectorsize).unwrap();
    assert!(
        matches!(ext, Extent::Inline { .. }),
        "a 1000-byte file should be inline, got {ext:?}"
    );

    let expected = f
        .staged
        .files
        .iter()
        .find(|(n, _)| n == "inline")
        .map(|(_, b)| b.clone())
        .unwrap();
    assert_eq!(f.read_all(ino, expected.len()), expected);
}

/// Each sector of file data is checked against the checksum tree, catching one
/// corrupted byte
#[test]
fn a_corrupted_data_sector_is_caught() {
    let mut f = fx!("rot");
    let ino = f.ino_of("big");

    // Where the first extent of that file actually is
    let key = Key { objectid: ino, kind: EXTENT_DATA_KEY, offset: 0 };
    let item = f.vol.find_item(f.tree, &key).expect("an extent record");
    let ext = parse_extent(&key, &item, f.vol.geometry.sectorsize).unwrap();
    let Extent::Regular { logical, .. } = ext else {
        panic!("a 1 MiB file should be a regular extent, got {ext:?}");
    };

    // It reads cleanly first, or this proves nothing
    let mut buf = [0u8; 4096];
    f.vol.read_file(f.tree, ino, 0, &mut buf).expect("must read before");

    // Flip a bit on the medium, under the checksum
    let phys = f.vol.chunks.map(logical, 0).unwrap();
    f.vol.host_mut().data[phys.offset as usize] ^= 0xFF;

    match f.vol.read_file(f.tree, ino, 0, &mut buf) {
        Err(BtrfsError::ChecksumMismatch) => {}
        other => panic!("a rotted sector must be caught, got {other:?}"),
    }
}

// hostile extents

fn reg_item(disk_bytenr: u64, offset: u64, num_bytes: u64, disk_num_bytes: u64) -> Vec<u8> {
    let mut v = vec![0u8; fe::SIZE_OF];
    v[fe::TYPE] = FILE_EXTENT_REG;
    v[fe::RAM_BYTES..fe::RAM_BYTES + 8].copy_from_slice(&num_bytes.to_le_bytes());
    v[fe::DISK_BYTENR..fe::DISK_BYTENR + 8].copy_from_slice(&disk_bytenr.to_le_bytes());
    v[fe::DISK_NUM_BYTES..fe::DISK_NUM_BYTES + 8].copy_from_slice(&disk_num_bytes.to_le_bytes());
    v[fe::OFFSET..fe::OFFSET + 8].copy_from_slice(&offset.to_le_bytes());
    v[fe::NUM_BYTES..fe::NUM_BYTES + 8].copy_from_slice(&num_bytes.to_le_bytes());
    v
}

const SS: u32 = 4096;

#[test]
fn a_well_formed_extent_parses() {
    let key = Key { objectid: 256, kind: EXTENT_DATA_KEY, offset: 0 };
    let item = reg_item(1 << 20, 0, 4096, 4096);
    match parse_extent(&key, &item, SS).expect("the control case must parse") {
        Extent::Regular { logical, num_bytes, .. } => {
            assert_eq!(logical, 1 << 20);
            assert_eq!(num_bytes, 4096);
        }
        other => panic!("{other:?}"),
    }
}

/// An extent record reaching past its extent would read what comes next
#[test]
fn a_record_reaching_past_its_extent_is_refused() {
    let key = Key { objectid: 256, kind: EXTENT_DATA_KEY, offset: 0 };
    assert!(parse_extent(&key, &reg_item(1 << 20, 4096, 8192, 8192), SS).is_err());
    assert!(parse_extent(&key, &reg_item(1 << 20, 0, 8192, 4096), SS).is_err());
}

#[test]
fn unaligned_extent_fields_are_refused() {
    let key = Key { objectid: 256, kind: EXTENT_DATA_KEY, offset: 0 };
    assert!(parse_extent(&key, &reg_item((1 << 20) + 1, 0, 4096, 4096), SS).is_err());
    assert!(parse_extent(&key, &reg_item(1 << 20, 1, 4096, 8192), SS).is_err());
    assert!(parse_extent(&key, &reg_item(1 << 20, 0, 4097, 8192), SS).is_err());

    // An unaligned file offset, in the key
    let bad_key = Key { objectid: 256, kind: EXTENT_DATA_KEY, offset: 1 };
    assert!(parse_extent(&bad_key, &reg_item(1 << 20, 0, 4096, 4096), SS).is_err());
}

#[test]
fn an_extent_whose_end_wraps_is_refused() {
    let key = Key { objectid: 256, kind: EXTENT_DATA_KEY, offset: 4096 };
    let item = reg_item(1 << 20, 0, u64::MAX - 4095, u64::MAX);
    assert!(parse_extent(&key, &item, SS).is_err());
}

#[test]
fn an_impossible_extent_type_is_refused() {
    let key = Key { objectid: 256, kind: EXTENT_DATA_KEY, offset: 0 };
    for bad in [FILE_EXTENT_TYPES, FILE_EXTENT_TYPES + 1, 200] {
        let mut item = reg_item(1 << 20, 0, 4096, 4096);
        item[fe::TYPE] = bad;
        assert!(parse_extent(&key, &item, SS).is_err(), "type {bad}");
    }
}

/// Compression is refused at the extent as well as the mount
#[test]
fn a_compressed_or_encrypted_extent_is_refused() {
    let key = Key { objectid: 256, kind: EXTENT_DATA_KEY, offset: 0 };
    for (field, val) in [(fe::COMPRESSION, 1u8), (fe::ENCRYPTION, 1)] {
        let mut item = reg_item(1 << 20, 0, 4096, 4096);
        item[field] = val;
        assert!(
            matches!(parse_extent(&key, &item, SS), Err(BtrfsError::Unsupported)),
            "field at {field} must be refused"
        );
    }
    let mut item = reg_item(1 << 20, 0, 4096, 4096);
    item[fe::OTHER_ENCODING..fe::OTHER_ENCODING + 2].copy_from_slice(&1u16.to_le_bytes());
    assert!(parse_extent(&key, &item, SS).is_err());
}

/// An inline extent's declared length and the item's size must agree
#[test]
fn an_inline_extent_whose_length_disagrees_with_its_item_is_refused() {
    let key = Key { objectid: 256, kind: EXTENT_DATA_KEY, offset: 0 };

    let mut good = vec![0u8; fe::INLINE_DATA_START + 10];
    good[fe::TYPE] = FILE_EXTENT_INLINE;
    good[fe::RAM_BYTES..fe::RAM_BYTES + 8].copy_from_slice(&10u64.to_le_bytes());
    parse_extent(&key, &good, SS).expect("the control case must parse");

    for claimed in [9u64, 11, 1 << 20, u64::MAX] {
        let mut bad = good.clone();
        bad[fe::RAM_BYTES..fe::RAM_BYTES + 8].copy_from_slice(&claimed.to_le_bytes());
        assert!(
            parse_extent(&key, &bad, SS).is_err(),
            "a claimed length of {claimed} against a 10-byte item must be refused"
        );
    }
}

/// An inline extent is the whole file, so at offset zero
#[test]
fn an_inline_extent_anywhere_but_the_start_is_refused() {
    let key = Key { objectid: 256, kind: EXTENT_DATA_KEY, offset: 4096 };
    let mut item = vec![0u8; fe::INLINE_DATA_START + 10];
    item[fe::TYPE] = FILE_EXTENT_INLINE;
    item[fe::RAM_BYTES..fe::RAM_BYTES + 8].copy_from_slice(&10u64.to_le_bytes());
    assert!(parse_extent(&key, &item, SS).is_err());
}

/// Preallocated space reads as zeros, not stale disk data
#[test]
fn a_preallocated_extent_is_not_read_from_the_disk() {
    let key = Key { objectid: 256, kind: EXTENT_DATA_KEY, offset: 0 };
    let mut item = reg_item(1 << 20, 0, 8192, 8192);
    item[fe::TYPE] = FILE_EXTENT_PREALLOC;
    match parse_extent(&key, &item, SS).unwrap() {
        Extent::Prealloc { num_bytes } => assert_eq!(num_bytes, 8192),
        other => panic!("prealloc must not decode as readable data: {other:?}"),
    }
}

/// An item too short for the header
#[test]
fn an_item_too_short_for_its_header_is_refused() {
    let key = Key { objectid: 256, kind: EXTENT_DATA_KEY, offset: 0 };
    for len in [0usize, 1, fe::INLINE_DATA_START - 1] {
        assert!(parse_extent(&key, &vec![0u8; len], SS).is_err(), "{len} bytes");
    }
    // A regular extent's item not exactly the struct
    for len in [fe::SIZE_OF - 1, fe::SIZE_OF + 1] {
        let mut item = reg_item(1 << 20, 0, 4096, 4096);
        item.resize(len, 0);
        assert!(parse_extent(&key, &item, SS).is_err(), "{len} bytes");
    }
}

/// The judges are present. Externally judged cases skip without `btrfs-progs`,
/// so this one fails, once per file. See `common::require_tools`
#[test]
fn the_external_judges_are_here() {
    common::require_tools();
}
