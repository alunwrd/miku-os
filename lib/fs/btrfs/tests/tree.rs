// Walking a real tree
//
//   1. the chunk tree read through the bootstrap map contains the same chunks
//      `btrfs inspect-internal dump-tree` lists
//   2. with those chunks in the map, the tree root becomes readable; it lives
//      in a metadata chunk the bootstrap array never described
//   3. a walk visits every item in key order and stops where it should
//
// `parse_node` verifies each block's crc32c, fsid and recorded address, so a
// wrong mapping or a mis-parsed item array cannot produce a plausible answer.

mod common;

use btrfs::error::BtrfsError;
use btrfs::structs::*;
use btrfs::{Key, Volume};
use common::Image;
use fshost::{BlockHost, HostError};
use std::path::Path;

/// The image file as a `BlockHost`, so `Volume` reads it as it reads a disk in
/// the kernel
struct FileHost {
    data: Vec<u8>,
}

impl FileHost {
    fn open(path: &Path) -> Self {
        FileHost { data: std::fs::read(path).expect("cannot read image") }
    }
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

fn volume(tag: &str) -> Option<(Image, Volume<FileHost>)> {
    let img = Image::new(tag, None)?;
    let host = FileHost::open(&img.path);
    let vol = Volume::open(host, 0).expect("a fresh volume must open");
    Some((img, vol))
}

macro_rules! fixture {
    ($tag:expr) => {
        match volume($tag) {
            Some(t) => t,
            None => {
                eprintln!("btrfs-progs not available - skipping");
                return;
            }
        }
    };
}

/// Opening reads the superblock and builds the bootstrap map, nothing more
#[test]
fn a_volume_opens_on_its_bootstrap_map() {
    let (_img, vol) = fixture!("open");
    assert_eq!(vol.geometry.sectorsize, 4096);
    assert_eq!(vol.geometry.nodesize, 16384);
    assert!(!vol.chunks.is_empty());
    for c in vol.chunks.iter() {
        assert!(c.is_system(), "only system chunks before the chunk tree is read");
    }
}

/// External judge: the chunks from walking the chunk tree and the chunks
/// `btrfs-progs` prints are the same set
#[test]
fn the_chunk_tree_holds_what_btrfs_progs_says_it_holds() {
    let (img, mut vol) = fixture!("chunktree");

    let mut ours: Vec<(u64, u64, u64)> = Vec::new();
    let sectorsize = vol.geometry.sectorsize;
    let root = vol.geometry.chunk_root;
    vol.for_each_item(root, &mut |key, data| {
        if key.kind == CHUNK_ITEM_KEY {
            let c = btrfs::chunk::parse_chunk(data, 0, key.offset, sectorsize)?;
            ours.push((c.start, c.length, c.ty));
        }
        Ok(true)
    })
    .expect("the chunk tree must walk");

    assert!(!ours.is_empty(), "a volume always has chunks");

    // `dump-tree -t chunk` prints a line per chunk: the key gives the start,
    // the body gives `length ... type`
    let dump = img.btrfs(&["inspect-internal", "dump-tree", "-t", "chunk"]);
    let mut theirs: Vec<u64> = Vec::new();
    for line in dump.lines() {
        if let Some(rest) = line.split("CHUNK_ITEM ").nth(1) {
            if let Some(num) = rest.split(')').next() {
                if let Ok(v) = num.trim().parse::<u64>() {
                    theirs.push(v);
                }
            }
        }
    }
    assert!(!theirs.is_empty(), "could not read dump-tree output:\n{dump}");

    let mut ours_starts: Vec<u64> = ours.iter().map(|c| c.0).collect();
    ours_starts.sort_unstable();
    theirs.sort_unstable();
    assert_eq!(
        ours_starts, theirs,
        "the chunks we walked are not the chunks btrfs-progs sees"
    );
}

/// End to end: the tree root is in a metadata chunk the bootstrap array never
/// described, so reading it needs a working map from the chunk tree walk
#[test]
fn the_tree_root_is_unreachable_until_the_chunk_tree_is_read() {
    let (_img, mut vol) = fixture!("reach");
    let root = vol.geometry.root;

    assert!(
        vol.read_node(root).is_err(),
        "the tree root cannot be in the bootstrap map - that map is system chunks"
    );

    vol.load_chunk_tree().expect("the chunk tree must load");

    let node = vol
        .read_node(root)
        .unwrap_or_else(|e| panic!("the tree root is still unreadable after loading chunks: {e:?}"));
    assert_eq!(node.bytenr, root, "the block knows its own address");
    assert_eq!(node.level, vol.geometry.root_level);
    assert!(node.nritems > 0, "the root tree is never empty");
}

/// A walk visits keys in order and nothing twice. A binary search over a node
/// depends on the order
#[test]
fn a_walk_visits_every_item_in_key_order() {
    let (_img, mut vol) = fixture!("order");
    vol.load_chunk_tree().unwrap();
    let root = vol.geometry.root;

    let mut seen: Vec<Key> = Vec::new();
    vol.for_each_item(root, &mut |k, _| {
        seen.push(k);
        Ok(true)
    })
    .expect("the root tree must walk");

    assert!(seen.len() > 1, "the root tree has several items");
    for pair in seen.windows(2) {
        assert!(
            pair[0] < pair[1],
            "keys came back out of order: {:?} then {:?}",
            pair[0],
            pair[1]
        );
    }
}

/// A callback that says stop is obeyed immediately, so a lookup costs one leaf
#[test]
fn a_walk_stops_when_it_is_told_to() {
    let (_img, mut vol) = fixture!("stop");
    vol.load_chunk_tree().unwrap();

    let mut count = 0;
    vol.for_each_item(vol.geometry.chunk_root, &mut |_, _| {
        count += 1;
        Ok(count < 2)
    })
    .unwrap();
    assert_eq!(count, 2, "the walk ran past the stop");
}

/// Starting part-way skips what is before and lands on the first key not less
/// than the one asked for
#[test]
fn a_walk_can_start_part_way_through() {
    let (_img, mut vol) = fixture!("from");
    vol.load_chunk_tree().unwrap();
    let root = vol.geometry.chunk_root;

    let mut all: Vec<Key> = Vec::new();
    vol.for_each_item(root, &mut |k, _| {
        all.push(k);
        Ok(true)
    })
    .unwrap();
    assert!(all.len() >= 3, "this case needs a few items, have {}", all.len());

    let from = all[all.len() / 2];
    let mut rest: Vec<Key> = Vec::new();
    vol.walk_from(root, &from, &mut |k, _| {
        rest.push(k);
        Ok(true)
    })
    .unwrap();

    assert_eq!(rest.first(), Some(&from), "the walk starts where it was told");
    assert_eq!(&rest[..], &all[all.len() / 2..], "and continues in order");
}

/// An exact lookup finds the item, with the same bytes the walk hands out for
/// that key
#[test]
fn an_exact_lookup_finds_the_item() {
    let (_img, mut vol) = fixture!("find");
    vol.load_chunk_tree().unwrap();
    let root = vol.geometry.chunk_root;

    let mut first: Option<(Key, Vec<u8>)> = None;
    vol.for_each_item(root, &mut |k, data| {
        first = Some((k, data.to_vec()));
        Ok(false)
    })
    .unwrap();
    let (key, bytes) = first.expect("the chunk tree has items");

    let found = vol.find_item(root, &key).expect("the key we just saw must be findable");
    assert_eq!(found, bytes, "the lookup and the walk disagree about the same key");
}

/// A missing key is `NotFound`, not the neighbouring item
#[test]
fn a_key_that_is_not_there_is_not_found() {
    let (_img, mut vol) = fixture!("missing");
    vol.load_chunk_tree().unwrap();

    let absent = Key { objectid: u64::MAX - 1, kind: 1, offset: 7 };
    assert!(matches!(
        vol.find_item(vol.geometry.chunk_root, &absent),
        Err(BtrfsError::NotFound)
    ));
}

/// Reading a block that is not a tree block fails. The superblock is a real,
/// checksummed structure at a known address, and not a node
#[test]
fn a_block_that_is_not_a_node_is_refused() {
    let (_img, mut vol) = fixture!("notanode");
    vol.load_chunk_tree().unwrap();

    // The first data address in the volume, not a tree block with that address
    // in its header
    let data_chunk = vol.chunks.iter().find(|c| c.is_data()).map(|c| c.start);
    if let Some(start) = data_chunk {
        assert!(
            vol.read_node(start).is_err(),
            "a data chunk's first block was parsed as a node"
        );
    }

    // And an address in no chunk at all
    assert!(matches!(vol.read_node(u64::MAX - 4095), Err(BtrfsError::NotFound)));
}

/// Every block the walk touches passed its own checksum, asserted directly
#[test]
fn every_block_in_the_tree_checksums() {
    let (_img, mut vol) = fixture!("csums");
    vol.load_chunk_tree().unwrap();

    for root in [vol.geometry.chunk_root, vol.geometry.root] {
        let node = vol.read_node(root).expect("a root must read");
        let stored = u32_at(node.bytes(), 0).unwrap();
        let computed = fshost::crc32c::crc32c(0, &node.bytes()[CSUM_SIZE..]);
        assert_eq!(stored, computed, "root {root} does not match its own checksum");

        // Its children one level down, where a mapping right for the root and
        // wrong further in would show
        if !node.is_leaf() {
            for slot in 0..node.nritems.min(8) {
                let child = node.blockptr_at(slot).unwrap();
                let c = vol
                    .read_node(child)
                    .unwrap_or_else(|e| panic!("child {slot} at {child}: {e:?}"));
                assert_eq!(c.bytenr, child);
            }
        }
    }
}

/// The judges are present. Externally judged cases skip without `btrfs-progs`,
/// so this one fails, once per file. See `common::require_tools`
#[test]
fn the_external_judges_are_here() {
    common::require_tools();
}
