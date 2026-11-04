// Copy-on-write and its three rules
//
// Nothing here touches the medium. A transaction builds the new tree in memory,
// and these checks read it back the way commit does.
//
// The allocator hands out addresses from a scratch range no real chunk uses.
// The real one reads the free space tree and writes the extent tree, themselves
// changed through this machinery; the trait keeps the two apart.
//
// The rules, from `src/trans.rs`:
//
//   1. a block freed by copy-on-write is not reused in the same transaction
//   2. a block is copied at most once per transaction
//   3. a copied parent points at the copied child
//
// Each is silent when broken. Rule 1 loses the filesystem on a power cut at the
// wrong moment; rule 2 leaks a block and leaves a parent pointing at a stale
// copy; rule 3 leaves the new tree pointing into the old one, which reads
// correctly at first.

mod common;

use btrfs::error::BtrfsError;
use btrfs::structs::*;
use btrfs::trans::{BlockAlloc, Trans};
use btrfs::Volume;
use common::Image;
use fshost::{BlockHost, HostError};
use std::collections::{HashMap, HashSet};

struct FileHost {
    data: Vec<u8>,
}

impl BlockHost for FileHost {
    fn read_sectors(&mut self, lba: u64, count: u32, buf: &mut [u8]) -> Result<(), HostError> {
        let s = (lba as usize) * 512;
        let e = s + (count as usize) * 512;
        let src = self.data.get(s..e).ok_or(HostError::Io)?;
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

/// Hands out addresses from an unused range and records everything it was told
/// to free
///
/// It does not honour rule 1 itself. The tests assert the rule against what the
/// transaction did, so a real allocator that gets it wrong fails here
struct Scratch {
    next: u64,
    step: u64,
    handed_out: Vec<u64>,
    freed: Vec<u64>,
    /// Set to hand out an address already given, to check the transaction
    /// catches a repeating allocator
    repeat_at: Option<usize>,
}

impl Scratch {
    fn new(nodesize: u64) -> Self {
        // Past any chunk of a 512 MiB volume, so it never collides with a real
        // block
        Self { next: 1 << 40, step: nodesize, handed_out: Vec::new(), freed: Vec::new(), repeat_at: None }
    }
}

impl BlockAlloc for Scratch {
    fn alloc(&mut self, size: u64) -> Result<u64, BtrfsError> {
        assert_eq!(size, self.step, "a metadata block is always one nodesize");
        if let Some(n) = self.repeat_at {
            if self.handed_out.len() == n && !self.handed_out.is_empty() {
                let repeat = self.handed_out[0];
                self.handed_out.push(repeat);
                return Ok(repeat);
            }
        }
        let a = self.next;
        self.next += self.step;
        self.handed_out.push(a);
        Ok(a)
    }
    fn free(&mut self, logical: u64, _size: u64) {
        self.freed.push(logical);
    }
}

struct Fixture {
    _img: Image,
    vol: Volume<FileHost>,
    /// A tree with more than one level, for the checks that need a parent
    deep_root: u64,
}

/// Enough names that the fs tree is more than one leaf. A fresh `mkfs` volume's
/// trees are one level deep, so `cow_path` never enters the internal-node
/// branch and the rule 3 check skips itself
const DEEP_FILES: usize = 4000;

fn fixture(tag: &str) -> Option<Fixture> {
    let mut stage = std::env::temp_dir();
    stage.push(format!("btrfs-trans-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&stage);
    std::fs::create_dir_all(&stage).ok()?;
    for i in 0..DEEP_FILES {
        std::fs::write(stage.join(format!("f{i:05}")), b"x").ok()?;
    }

    let img = Image::new(tag, Some(&stage))?;
    let _ = std::fs::remove_dir_all(&stage);

    let host = FileHost { data: std::fs::read(&img.path).expect("cannot read image") };
    let mut vol = Volume::open(host, 0).expect("a fresh volume must open");
    vol.load_chunk_tree().expect("the chunk tree must load");

    let fs_root = vol.fs_root().expect("the fs tree must be findable");
    assert!(
        fs_root.level >= 1,
        "this fixture exists to give a multi-level tree and gave a level-{} one -          raise DEEP_FILES",
        fs_root.level
    );
    Some(Fixture { _img: img, vol, deep_root: fs_root.bytenr })
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
    fn trans(&mut self) -> Trans<Scratch> {
        let g = self.vol.geometry;
        let fsid = *<&[u8; FSID_SIZE]>::try_from(self.vol.superblock.fsid()).unwrap();
        Trans::new(
            Scratch::new(g.nodesize as u64),
            g.nodesize,
            g.sectorsize,
            fsid,
            g.generation + 1,
        )
    }

    /// Read a block straight off the medium, which is what the transaction
    /// falls through to
    fn reader(&mut self) -> impl FnMut(u64) -> Result<Vec<u8>, BtrfsError> + '_ {
        let nodesize = self.vol.geometry.nodesize as usize;
        let vol = &mut self.vol;
        move |logical| {
            let mut buf = vec![0u8; nodesize];
            vol.read_logical(logical, &mut buf)?;
            Ok(buf)
        }
    }
}

/// Walk the new tree out of a sealed transaction, falling through to the medium
/// for untouched blocks. Same shape as the commit
fn walk_new(
    sealed: &HashMap<u64, Vec<u8>>,
    medium: &mut dyn FnMut(u64) -> Result<Vec<u8>, BtrfsError>,
    root: u64,
    fsid: &[u8; FSID_SIZE],
    sectorsize: u32,
    out: &mut Vec<(Key, Vec<u8>)>,
    depth: usize,
) -> Result<(), BtrfsError> {
    assert!(depth < 16, "a tree that deep is a loop");
    let data = match sealed.get(&root) {
        Some(d) => d.clone(),
        None => medium(root)?,
    };
    // Sealed blocks carry their checksum and the WRITTEN flag, so the ordinary
    // parser applies: the new tree has to satisfy the reader that reads a disk
    let node = btrfs::node::parse_node(data, root, fsid, sectorsize)?;
    if node.is_leaf() {
        for slot in 0..node.nritems {
            out.push((node.key_at(slot)?, node.item_data(slot)?.to_vec()));
        }
        return Ok(());
    }
    for slot in 0..node.nritems {
        walk_new(sealed, medium, node.blockptr_at(slot)?, fsid, sectorsize, out, depth + 1)?;
    }
    Ok(())
}

/// The whole chunk tree as `(key, bytes)`, read the ordinary way
fn read_all(vol: &mut Volume<FileHost>, root: u64) -> Vec<(Key, Vec<u8>)> {
    let mut out = Vec::new();
    vol.for_each_item(root, &mut |k, d| {
        out.push((k, d.to_vec()));
        Ok(true)
    })
    .expect("the tree must walk");
    out
}

/// Copying a path returns blocks that all belong to this transaction, and the
/// tree under the new root stays a tree
#[test]
fn copying_a_path_produces_a_valid_tree() {
    let mut f = fx!("cowpath");
    let root = f.deep_root;
    let before = read_all(&mut f.vol, root);
    assert!(!before.is_empty());

    let key = before[before.len() / 2].0;
    let mut t = f.trans();
    let mut read = f.reader();
    let path = t.cow_path(root, &key, &mut read).expect("the path must copy");
    drop(read);

    assert!(path.depth() >= 1);
    for step in &path.steps {
        assert!(t.is_dirty(step.logical), "every block on the path is ours");
    }
    let new_root = t.new_root_of(root).expect("the root moved");
    assert_eq!(new_root, path.steps[0].logical);
    assert_ne!(new_root, root, "the root was copied, not modified in place");

    // The new tree holds what the old one did
    let sealed: HashMap<u64, Vec<u8>> = t.seal().unwrap().into_iter().collect();
    let fsid = *<&[u8; FSID_SIZE]>::try_from(f.vol.superblock.fsid()).unwrap();
    let ss = f.vol.geometry.sectorsize;
    let mut after = Vec::new();
    let mut medium = f.reader();
    walk_new(&sealed, &mut medium, new_root, &fsid, ss, &mut after, 0)
        .expect("the new tree must walk");
    drop(medium);
    assert_eq!(after, before, "copying a path changed what the tree holds");
}

/// Rule 3: a copied parent points at the copied child. Otherwise the new tree
/// points into the old one, which reads correctly until the old block is reused
#[test]
fn a_copied_parent_points_at_the_copied_child() {
    let mut f = fx!("parent");
    // The fs tree has levels. The chunk tree on a small volume is a single leaf
    // and would skip this check
    let root = f.deep_root;
    let before = read_all(&mut f.vol, root);
    let key = before[before.len() / 2].0;

    let mut t = f.trans();
    let mut read = f.reader();
    let path = t.cow_path(root, &key, &mut read).expect("the path must copy");
    drop(read);

    assert!(
        path.depth() >= 2,
        "the fixture promised a multi-level tree and gave a path of depth {}",
        path.depth()
    );
    let sealed: HashMap<u64, Vec<u8>> = t.seal().unwrap().into_iter().collect();
    let fsid = *<&[u8; FSID_SIZE]>::try_from(f.vol.superblock.fsid()).unwrap();
    let ss = f.vol.geometry.sectorsize;

    for w in path.steps.windows(2) {
        let (parent, child) = (w[0], w[1]);
        let data = sealed.get(&parent.logical).expect("the parent is sealed").clone();
        let node = btrfs::node::parse_node(data, parent.logical, &fsid, ss).unwrap();
        assert_eq!(
            node.blockptr_at(parent.slot).unwrap(),
            child.logical,
            "the parent at slot {} points at the old child, not the copy",
            parent.slot
        );
    }
}

/// Rule 2: touching the same leaf twice copies it once. A second copy leaks the
/// first and leaves the parent pointing at it
#[test]
fn a_block_is_copied_at_most_once() {
    let mut f = fx!("once");
    let root = f.deep_root;
    let before = read_all(&mut f.vol, root);
    assert!(before.len() >= 2, "this case needs a tree with a few items");

    let mut t = f.trans();
    let mut read = f.reader();
    let first = t.cow_path(root, &before[0].0, &mut read).unwrap();
    let count_after_one = t.dirty_count();
    let second = t.cow_path(root, &before[1].0, &mut read).unwrap();
    drop(read);

    // Both keys share a leaf on a small volume, so the second path copied
    // nothing
    if first.leaf().unwrap().logical == second.leaf().unwrap().logical {
        assert_eq!(
            t.dirty_count(),
            count_after_one,
            "the second path copied blocks that were already ours"
        );
    }
    assert_eq!(
        first.steps[0].logical, second.steps[0].logical,
        "two paths in one transaction must share one root"
    );
}

/// Rule 1: a block the transaction copied away from does not come back as a new
/// allocation. Its old contents are what a crash falls back to
#[test]
fn a_freed_block_is_never_handed_out_again() {
    let mut f = fx!("rule1");
    let root = f.deep_root;
    let before = read_all(&mut f.vol, root);

    let mut t = f.trans();
    let mut read = f.reader();
    for (k, _) in before.iter().take(4) {
        t.cow_path(root, k, &mut read).unwrap();
    }
    drop(read);

    let freed: HashSet<u64> = t.freed_blocks().collect();
    assert!(!freed.is_empty(), "copying a path frees the block it copied");

    let handed: HashSet<u64> = t.alloc.handed_out.iter().copied().collect();
    let overlap: Vec<u64> = freed.intersection(&handed).copied().collect();
    assert!(
        overlap.is_empty(),
        "these blocks were freed and handed out again in one transaction: {overlap:?}"
    );

    // Nothing the old tree still uses is in the dirty set under its own
    // address: every new block is at a new address
    for (logical, _) in t.dirty_blocks() {
        assert!(
            !freed.contains(&logical),
            "block {logical} was copied away from and is also a new block"
        );
    }
}

/// An allocator handing back an address this transaction holds is an allocator
/// bug, and the transaction refuses to write two blocks to one place
#[test]
fn an_allocator_that_repeats_itself_is_caught() {
    let mut f = fx!("repeat");
    let root = f.deep_root;
    let key = read_all(&mut f.vol, root)[0].0;

    let mut t = f.trans();
    t.alloc.repeat_at = Some(1);
    let mut read = f.reader();
    let r = t.cow_path(root, &key, &mut read);
    drop(read);

    // The tree has levels, so the second allocation happens and the repeat
    // fires
    assert!(
        r.is_err(),
        "an allocator that handed back an address already in use was accepted"
    );
}

/// The smallest change: an item's bytes, same size, nothing moves. The new tree
/// holds the new bytes and nothing else differs
#[test]
fn an_item_can_be_changed_in_place() {
    let mut f = fx!("update");
    let root = f.deep_root;
    let before = read_all(&mut f.vol, root);
    // From the middle, so the path crosses a parent and does not land in the
    // leftmost leaf
    let (key, old) = before[before.len() / 2].clone();

    // Flip one byte of the item's data; any byte will do
    let mut new_bytes = old.clone();
    new_bytes[0] ^= 0xFF;

    let mut t = f.trans();
    let mut read = f.reader();
    t.update_item(root, &key, &new_bytes, &mut read)
        .expect("an in-place update must work");
    drop(read);

    let new_root = t.new_root_of(root).expect("the root moved");
    let sealed: HashMap<u64, Vec<u8>> = t.seal().unwrap().into_iter().collect();
    let fsid = *<&[u8; FSID_SIZE]>::try_from(f.vol.superblock.fsid()).unwrap();
    let ss = f.vol.geometry.sectorsize;

    let mut after = Vec::new();
    let mut medium = f.reader();
    walk_new(&sealed, &mut medium, new_root, &fsid, ss, &mut after, 0)
        .expect("the new tree must walk");
    drop(medium);

    assert_eq!(after.len(), before.len(), "the number of items changed");
    for (i, ((ka, da), (kb, db))) in after.iter().zip(before.iter()).enumerate() {
        assert_eq!(ka, kb, "item {i}: the keys changed");
        if i == before.len() / 2 {
            assert_eq!(da, &new_bytes, "the changed item does not hold the new bytes");
        } else {
            assert_eq!(da, db, "item {i} changed and should not have");
        }
    }
}

/// A resize is not an in-place update. Treating it as one truncates
#[test]
fn an_update_that_changes_the_size_is_refused() {
    let mut f = fx!("resize");
    let root = f.vol.geometry.chunk_root;
    let (key, old) = read_all(&mut f.vol, root)[0].clone();

    for len in [old.len() - 1, old.len() + 1, 0] {
        let mut t = f.trans();
        let mut read = f.reader();
        let r = t.update_item(root, &key, &vec![0u8; len], &mut read);
        drop(read);
        assert!(
            matches!(r, Err(BtrfsError::InvalidArg)),
            "{len} bytes against a {}-byte item must be refused",
            old.len()
        );
    }
}

/// A missing key changes nothing: no insert, no write over the neighbouring
/// item
#[test]
fn updating_a_key_that_is_not_there_is_not_found() {
    let mut f = fx!("absent");
    let root = f.vol.geometry.chunk_root;
    let absent = Key { objectid: u64::MAX - 3, kind: 1, offset: 9 };

    let mut t = f.trans();
    let mut read = f.reader();
    let r = t.update_item(root, &absent, &[0u8; 8], &mut read);
    drop(read);
    assert!(matches!(r, Err(BtrfsError::NotFound)));
}

/// Sealing computes each block's checksum last, over its final bytes. An
/// earlier checksum would be refused on the next mount
#[test]
fn sealed_blocks_satisfy_the_ordinary_reader() {
    let mut f = fx!("seal");
    let root = f.deep_root;
    let all = read_all(&mut f.vol, root);
    let (key, old) = all[all.len() / 2].clone();
    let mut new_bytes = old.clone();
    new_bytes[0] ^= 0xFF;

    let mut t = f.trans();
    let mut read = f.reader();
    t.update_item(root, &key, &new_bytes, &mut read).unwrap();
    drop(read);

    let fsid = *<&[u8; FSID_SIZE]>::try_from(f.vol.superblock.fsid()).unwrap();
    let ss = f.vol.geometry.sectorsize;
    let sealed = t.seal().unwrap();
    assert!(!sealed.is_empty());

    for (logical, data) in sealed {
        // The ordinary parser: checksum, fsid, own address, WRITTEN flag, and
        // every structural rule
        let node = btrfs::node::parse_node(data.clone(), logical, &fsid, ss)
            .unwrap_or_else(|e| panic!("block {logical} would be refused on a mount: {e:?}"));
        assert_eq!(node.bytenr, logical);
        assert_eq!(
            node.generation,
            f.vol.geometry.generation + 1,
            "a block written by this transaction carries its generation"
        );
        let stored = u32_at(&data, 0).unwrap();
        assert_eq!(stored, fshost::crc32c::crc32c(0, &data[CSUM_SIZE..]));
    }
}

/// Nothing above wrote anything. The medium stays byte-for-byte the same, since
/// the old tree has to be intact until a superblock says otherwise
#[test]
fn a_transaction_writes_nothing() {
    let mut f = fx!("nowrite");
    let before = f.vol.host_mut().data.clone();

    let root = f.deep_root;
    let all = read_all(&mut f.vol, root);
    let (key, old) = all[all.len() / 2].clone();
    let mut new_bytes = old.clone();
    new_bytes[0] ^= 0xFF;

    let mut t = f.trans();
    let mut read = f.reader();
    t.update_item(root, &key, &new_bytes, &mut read).unwrap();
    drop(read);
    let _ = t.seal().unwrap();

    assert!(
        f.vol.host_mut().data == before,
        "a transaction changed the medium before anything was committed"
    );
}

/// The judges are present. Externally judged cases skip without `btrfs-progs`,
/// so this one fails, once per file. See `common::require_tools`
#[test]
fn the_external_judges_are_here() {
    common::require_tools();
}
