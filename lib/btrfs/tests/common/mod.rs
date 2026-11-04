// A real btrfs volume, for tests to read
//
// Made by `mkfs.btrfs`, not by hand: a hand-built image only checks this crate
// against its own assumptions.
//
// The tools are vendored in `refsrc/btrfs-progs` and built there, since the
// system has no btrfs-progs package; PATH is the fallback. With neither, a test
// skips, and `require_tools` keeps that visible.

#![allow(dead_code)]

use btrfs::space::{
    FREE_SPACE_BITMAP_KEY, FREE_SPACE_EXTENT_KEY, FREE_SPACE_INFO_KEY, FREE_SPACE_TREE_OBJECTID,
    FREE_SPACE_USING_BITMAPS,
};
use btrfs::structs::{u32_at, SUPER_INFO_OFFSET};
use btrfs::{Key, Superblock, Volume};
use fshost::{BlockHost, HostError};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::Command;

pub fn tool(name: &str) -> Option<PathBuf> {
    let vendored = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../refsrc/btrfs-progs")
        .join(name);
    if vendored.exists() {
        return Some(vendored);
    }
    let out = Command::new(name).arg("--version").output().ok()?;
    out.status.success().then(|| PathBuf::from(name))
}

/// Every tool the fixtures and judges need
///
/// `mkfs.btrfs` builds every image; `btrfs` reads back what we wrote with
/// `inspect-internal dump-tree`, `dump-super` and `filesystem show`; `btrfsck`
/// is the cross-checking judge
pub const REQUIRED_TOOLS: [&str; 3] = ["mkfs.btrfs", "btrfs", "btrfsck"];

/// Which of them are missing
pub fn missing_tools() -> Vec<&'static str> {
    REQUIRED_TOOLS.into_iter().filter(|t| tool(t).is_none()).collect()
}

/// Makes an absent toolchain visible
///
/// Every externally judged case returns `None` and stops when its tool is
/// missing. With neither `refsrc/btrfs-progs` nor a system btrfs-progs, all of
/// them skip and the suite still reports success.
///
/// So each test file that uses them calls this once, from a case of its own,
/// and it fails with the reason and the fix.
///
/// `BTRFS_ALLOW_NO_PROGS=1` turns it off, for the arithmetic-only half of the
/// suite
pub fn require_tools() {
    if std::env::var_os("BTRFS_ALLOW_NO_PROGS").is_some() {
        return;
    }
    let missing = missing_tools();
    assert!(
        missing.is_empty(),
        "btrfs-progs not found: {missing:?}.\n\
         Without it every case in this crate that judges our bytes against \
         btrfs's own reading of them skips, and the suite reports success.\n\
         Fix: build the vendored copy in refsrc/btrfs-progs, or install \
         btrfs-progs, or set BTRFS_ALLOW_NO_PROGS=1 to accept the gap."
    );
}

/// 512 MiB: past the minimum volume size, and holds the first backup superblock
/// at 64 MiB
pub const IMG_BYTES: u64 = 512 * 1024 * 1024;

pub struct Image {
    pub path: PathBuf,
}

impl Image {
    /// A fresh volume. `populate` names a directory `mkfs -r` copies in, which
    /// needs no root and no mount, like `mke2fs -d` in the ext tests
    ///
    /// `None` means the tool is absent. An `mkfs` that runs and fails panics
    pub fn new(tag: &str, populate: Option<&Path>) -> Option<Self> {
        let mkfs = tool("mkfs.btrfs")?;
        let mut path = std::env::temp_dir();
        path.push(format!("btrfs-{tag}-{}.img", std::process::id()));

        let f = std::fs::File::create(&path).expect("cannot create image");
        f.set_len(IMG_BYTES).expect("cannot size image");
        drop(f);

        let mut cmd = Command::new(&mkfs);
        cmd.arg("-q");
        if let Some(dir) = populate {
            cmd.arg("-r").arg(dir);
        }
        cmd.arg(&path);
        let out = cmd.output().expect("mkfs.btrfs did not run");
        assert!(
            out.status.success(),
            "mkfs.btrfs failed: {}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        Some(Image { path })
    }

    pub fn read_at(&self, offset: u64, buf: &mut [u8]) {
        let mut f = std::fs::File::open(&self.path).expect("cannot open image");
        f.seek(SeekFrom::Start(offset)).expect("cannot seek");
        f.read_exact(buf).expect("cannot read");
    }

    pub fn read_super_at(&self, offset: u64) -> Superblock {
        let mut sb = Superblock::zeroed();
        self.read_at(offset, &mut sb.data);
        sb
    }

    pub fn read_super(&self) -> Superblock {
        self.read_super_at(SUPER_INFO_OFFSET)
    }

    /// What btrfs-progs says, for a question this crate answers the same way
    pub fn btrfs(&self, args: &[&str]) -> String {
        let btrfs = tool("btrfs").expect("btrfs tool vanished");
        let out = Command::new(btrfs)
            .args(args)
            .arg(&self.path)
            .output()
            .expect("btrfs did not run");
        let mut s = String::from_utf8_lossy(&out.stdout).into_owned();
        s.push_str(&String::from_utf8_lossy(&out.stderr));
        s
    }
}

impl Drop for Image {
    fn drop(&mut self) {
        if std::env::var_os("BTRFS_KEEP_IMAGES").is_some() {
            eprintln!("kept image: {}", self.path.display());
            return;
        }
        let _ = std::fs::remove_file(&self.path);
    }
}

// The bitmap representation
//
// A real volume's free space tree, re-encoded from one item per run into
// bitmaps. `tests/space.rs` reads the result and `tests/alloc.rs` writes to it.
//
// `mkfs.btrfs` cannot be asked for bitmaps: a fresh volume never has enough
// free runs to cross the threshold.

/// A volume open over an in-memory image, read-only. Rewriting the free space
/// tree needs the chunk map to find every copy of a block
struct MemHost {
    data: Vec<u8>,
}

impl BlockHost for MemHost {
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

fn volume_over(bytes: &[u8]) -> Volume<MemHost> {
    let mut vol = Volume::open(MemHost { data: bytes.to_vec() }, 0).expect("the volume must open");
    vol.load_chunk_tree().expect("the chunk tree must load");
    vol
}

const HDR: usize = 101;
const ITEM: usize = 25;

/// Rewrite the free space tree's leaf into the bitmap representation, in every
/// copy the chunk holds
///
/// Panics when the tree is deeper than one leaf: an interior node is not
/// rebuilt, and a silent `None` would take the bitmap decoder out of the suite
pub fn into_bitmaps(bytes: &[u8]) -> Vec<u8> {
    let mut vol = volume_over(bytes);
    let root = vol
        .find_root(FREE_SPACE_TREE_OBJECTID)
        .expect("the free space tree must have a root item");
    assert_eq!(
        root.level, 0,
        "the free space tree of this fixture is {} levels deep - make the fixture smaller \
         rather than letting the bitmap case stop running",
        root.level
    );

    let nodesize = vol.geometry.nodesize as usize;
    let sector = vol.geometry.sectorsize as u64;
    let chunk = vol.chunks.find(root.bytenr).expect("the leaf must be in a chunk");
    let copies = chunk.profile.copies();
    let places: Vec<usize> = (0..copies)
        .map(|m| vol.chunks.map(root.bytenr, m).expect("a copy must map").offset as usize)
        .collect();

    let leaf = &bytes[places[0]..places[0] + nodesize];
    let nritems = u32_at(leaf, 96).unwrap() as usize;

    // Read the leaf as it stands: each group's info item, then its free runs
    let mut groups: Vec<(u64, u64, u32, Vec<(u64, u64)>)> = Vec::new();
    for i in 0..nritems {
        let p = HDR + i * ITEM;
        let key = Key::read(leaf, p).unwrap();
        let off = u32_at(leaf, p + 17).unwrap() as usize;
        let size = u32_at(leaf, p + 21).unwrap() as usize;
        let data = &leaf[HDR + off..HDR + off + size];
        match key.kind {
            FREE_SPACE_INFO_KEY => {
                groups.push((key.objectid, key.offset, u32_at(data, 0).unwrap(), Vec::new()))
            }
            FREE_SPACE_EXTENT_KEY => {
                groups.last_mut().unwrap().3.push((key.objectid, key.offset))
            }
            other => panic!("this fixture already holds key type {other}"),
        }
    }

    // Write it back the other way: the same runs, as bits
    let mut items: Vec<(Key, Vec<u8>)> = Vec::new();
    let range = sector * 2048; // what one 256-byte bitmap item covers
    for (start, length, count, runs) in &groups {
        let mut info = Vec::from(count.to_le_bytes());
        info.extend_from_slice(&FREE_SPACE_USING_BITMAPS.to_le_bytes());
        items.push((Key { objectid: *start, kind: FREE_SPACE_INFO_KEY, offset: *length }, info));

        let mut at = *start;
        while at < start + length {
            let len = range.min(start + length - at);
            let bits = len / sector;
            let mut bm = vec![0u8; ((bits + 7) / 8) as usize];
            for &(rs, rl) in runs {
                let from = rs.max(at);
                let to = (rs + rl).min(at + len);
                let mut b = from;
                while b < to {
                    let idx = ((b - at) / sector) as usize;
                    bm[idx / 8] |= 1 << (idx % 8);
                    b += sector;
                }
            }
            items.push((Key { objectid: at, kind: FREE_SPACE_BITMAP_KEY, offset: len }, bm));
            at += len;
        }
    }

    let mut out = vec![0u8; nodesize];
    out[..HDR].copy_from_slice(&leaf[..HDR]);
    let mut dend = nodesize - HDR;
    for (i, (key, data)) in items.iter().enumerate() {
        dend -= data.len();
        assert!(HDR + (i + 1) * ITEM <= HDR + dend, "the rewritten leaf does not fit");
        let p = HDR + i * ITEM;
        out[p..p + 8].copy_from_slice(&key.objectid.to_le_bytes());
        out[p + 8] = key.kind;
        out[p + 9..p + 17].copy_from_slice(&key.offset.to_le_bytes());
        out[p + 17..p + 21].copy_from_slice(&(dend as u32).to_le_bytes());
        out[p + 21..p + 25].copy_from_slice(&(data.len() as u32).to_le_bytes());
        out[HDR + dend..HDR + dend + data.len()].copy_from_slice(data);
    }
    out[96..100].copy_from_slice(&(items.len() as u32).to_le_bytes());
    let csum = fshost::crc32c::crc32c(0, &out[32..]);
    out[..4].copy_from_slice(&csum.to_le_bytes());

    let mut image = bytes.to_vec();
    for place in places {
        image[place..place + nodesize].copy_from_slice(&out);
    }
    image
}

