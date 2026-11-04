// Changing a tree's shape, and ways a leaf can be built wrong
//
// `src/shape.rs` does item arithmetic over a block growing from both ends,
// whose mistakes leave plausible bytes. So each block goes through the parser
// that later judges it:
//
//   * in flight: `parse_node_in_flight`, `__btrfs_check_leaf` and
//     `__btrfs_check_node` minus two questions wrong for a
//     half-built block. It checks the data region's tiling after every operation.
//   * sealed: `parse_node`, as a mount uses.
//
// Beyond one block:
//
//   * a parent's key is its child's first key. `check_node` does not check
//     this, `btrfs check` does; it catches a
//     split key from the wrong side.
//   * keys ascend strictly across the whole walk.
//   * the keys reachable from the root are exactly those expected.
//
// Nothing here skips. btrfs-progs is vendored and built in
// `refsrc/btrfs-progs`, so its absence is a broken checkout.

mod common;

use btrfs::error::BtrfsError;
use btrfs::node::{parse_node, parse_node_in_flight};
use btrfs::structs::*;
use btrfs::trans::{BlockAlloc, Trans};
use btrfs::Volume;
use common::Image;
use fshost::{BlockHost, HostError};
use std::collections::{BTreeMap, HashMap, HashSet};

// The host and allocator, as in `tests/trans.rs`

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

/// Addresses from a range no chunk on 512 MiB reaches, and a record of what it
/// was asked. It ignores rule 1 itself, so checks are against the transaction.
/// `hand_back_pinned` breaks the rule on purpose
struct Scratch {
    next: u64,
    step: u64,
    handed_out: Vec<u64>,
    freed: Vec<u64>,
    /// Gives this address instead of a fresh one, once
    hand_back_pinned: Option<u64>,
}

impl Scratch {
    fn new(nodesize: u64) -> Self {
        Self {
            next: 1 << 40,
            step: nodesize,
            handed_out: Vec::new(),
            freed: Vec::new(),
            hand_back_pinned: None,
        }
    }
}

impl BlockAlloc for Scratch {
    fn alloc(&mut self, size: u64) -> Result<u64, BtrfsError> {
        assert_eq!(size, self.step, "a metadata block is always one nodesize");
        if let Some(a) = self.hand_back_pinned.take() {
            self.handed_out.push(a);
            return Ok(a);
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

// The fixture

/// Enough names that the fs tree is not one leaf, as in `tests/trans.rs`: real
/// parents, leaves, and item sizes. `mkfs -r` leaves are full, so the first
/// insert must split or push
const DEEP_FILES: usize = 4000;

struct Fixture {
    _img: Image,
    vol: Volume<FileHost>,
    /// The fs tree: more than one level, with packed leaves
    deep_root: u64,
    /// The chunk tree: one leaf at this size, reaching "the root is a leaf and
    /// it filled up"
    flat_root: u64,
    fsid: [u8; FSID_SIZE],
    nodesize: u32,
    sectorsize: u32,
    generation: u64,
}

fn fixture(tag: &str) -> Fixture {
    let mut stage = std::env::temp_dir();
    stage.push(format!("btrfs-shape-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&stage);
    std::fs::create_dir_all(&stage).expect("cannot make the staging directory");
    for i in 0..DEEP_FILES {
        std::fs::write(stage.join(format!("f{i:05}")), b"x").expect("cannot stage a file");
    }

    let img = Image::new(tag, Some(&stage)).expect(
        "mkfs.btrfs is missing. It is vendored at refsrc/btrfs-progs and built there; \
         a checkout without it cannot run this file, and a case that quietly passed \
         without a fixture is the bug this message exists to prevent",
    );
    let _ = std::fs::remove_dir_all(&stage);

    let host = FileHost { data: std::fs::read(&img.path).expect("cannot read image") };
    let mut vol = Volume::open(host, 0).expect("a fresh volume must open");
    vol.load_chunk_tree().expect("the chunk tree must load");

    let fs_root = vol.fs_root().expect("the fs tree must be findable");
    assert!(
        fs_root.level >= 1,
        "this fixture exists to give a multi-level tree and gave a level-{} one - \
         raise DEEP_FILES",
        fs_root.level
    );
    let g = vol.geometry;
    let flat = vol.read_node(g.chunk_root).expect("the chunk tree root must parse");
    assert_eq!(
        flat.level, 0,
        "the chunk tree on a volume this size is one leaf, and the cases that grow a \
         root from a leaf need it to be"
    );
    let fsid = *<&[u8; FSID_SIZE]>::try_from(vol.superblock.fsid()).unwrap();

    Fixture {
        _img: img,
        vol,
        deep_root: fs_root.bytenr,
        flat_root: g.chunk_root,
        fsid,
        nodesize: g.nodesize,
        sectorsize: g.sectorsize,
        generation: g.generation,
    }
}

impl Fixture {
    fn trans(&self) -> Trans<Scratch> {
        Trans::new(
            Scratch::new(self.nodesize as u64),
            self.nodesize,
            self.sectorsize,
            self.fsid,
            self.generation + 1,
        )
    }

    fn reader(&mut self) -> impl FnMut(u64) -> Result<Vec<u8>, BtrfsError> + '_ {
        let nodesize = self.nodesize as usize;
        let vol = &mut self.vol;
        move |logical| {
            let mut buf = vec![0u8; nodesize];
            vol.read_logical(logical, &mut buf)?;
            Ok(buf)
        }
    }

    /// Everything a tree holds, read the ordinary way off the medium
    fn read_all(&mut self, root: u64) -> Vec<(Key, Vec<u8>)> {
        let mut out = Vec::new();
        self.vol
            .for_each_item(root, &mut |k, d| {
                out.push((k, d.to_vec()));
                Ok(true)
            })
            .expect("the tree must walk");
        out
    }
}

// Walking a tree under construction, checking everything

#[derive(Default)]
struct Walk {
    items: Vec<(Key, Vec<u8>)>,
    max_level: u8,
    leaves: usize,
    blocks: usize,
    last: Option<Key>,
}

/// Walks the tree under `root`, asserting each structural rule and collecting
/// its contents. `expect_first` is the parent's key for this block, catching a
/// split key from the wrong side
fn walk(
    t: &Trans<Scratch>,
    read: &mut dyn FnMut(u64) -> Result<Vec<u8>, BtrfsError>,
    logical: u64,
    expect_first: Option<Key>,
    depth: usize,
    w: &mut Walk,
) {
    assert!(depth < MAX_LEVEL as usize + 1, "a tree that deep is a loop");
    let node = t
        .node_at(logical, read)
        .unwrap_or_else(|e| panic!("block {logical} would be refused: {e:?}"));
    w.blocks += 1;
    w.max_level = w.max_level.max(node.level);

    if let Some(k) = expect_first {
        assert!(
            node.nritems > 0,
            "block {logical} is empty and something points at it; only a root may be empty"
        );
        assert_eq!(
            node.key_at(0).unwrap(),
            k,
            "block {logical}: its parent's key is not its first key. A node's key is the \
             smallest key beneath it, and a search that trusts a wrong one returns the \
             wrong item rather than failing"
        );
    }

    if node.is_leaf() {
        w.leaves += 1;
        for slot in 0..node.nritems {
            let k = node.key_at(slot).unwrap();
            if let Some(prev) = w.last {
                assert!(
                    prev < k,
                    "keys do not ascend across the tree: {prev:?} then {k:?} in block {logical}"
                );
            }
            w.last = Some(k);
            w.items.push((k, node.item_data(slot).unwrap().to_vec()));
        }
        return;
    }

    for slot in 0..node.nritems {
        let child = node.blockptr_at(slot).unwrap();
        let k = node.key_at(slot).unwrap();
        walk(t, read, child, Some(k), depth + 1, w);
    }
}

/// Each block this transaction will write, through the mount's checker.
/// `parse_node_in_flight`: no checksum, `WRITTEN` clear, shape checked as for a
/// disk block
fn check_every_dirty_block(t: &Trans<Scratch>, sectorsize: u32) {
    let mut n = 0usize;
    for (logical, data) in t.dirty_blocks() {
        parse_node_in_flight(data.clone(), logical, sectorsize).unwrap_or_else(|e| {
            panic!("block {logical}, built by this transaction, is not a valid block: {e:?}")
        });
        n += 1;
    }
    assert!(n > 0, "nothing was built, so nothing was checked");
}

/// By hand what `parse_node_in_flight` asserts by parsing: a leaf's items tile
/// the data region exactly, and the item array never reaches the data
fn assert_leaf_tiles(data: &[u8], logical: u64) {
    let level = data[100];
    if level != 0 {
        return;
    }
    let capacity = (data.len() - HEADER_SIZE) as u32;
    let n = u32_at(data, 96).unwrap();
    let mut expected_end = capacity;
    for slot in 0..n {
        let at = HEADER_SIZE + ITEM_SIZE * slot as usize;
        let off = u32_at(data, at + 17).unwrap();
        let size = u32_at(data, at + 21).unwrap();
        assert_eq!(
            off + size,
            expected_end,
            "leaf {logical} slot {slot}: its data ends at {} and the item before it \
             starts at {expected_end} - a gap or an overlap either way",
            off + size
        );
        assert!(off + size <= capacity, "leaf {logical} slot {slot}: data past the block");
        assert!(
            off as usize >= ITEM_SIZE * (slot as usize + 1),
            "leaf {logical} slot {slot}: the item array has reached the data"
        );
        expected_end = off;
    }
}

/// A key space no real item uses, so a synthetic item does not collide. 254 is
/// no format key type
fn synth(n: u64) -> Key {
    Key { objectid: 0x7000_0000_0000_0000 + n, kind: 254, offset: n }
}

fn body_of(n: u64, len: usize) -> Vec<u8> {
    (0..len).map(|i| (n as u8).wrapping_add(i as u8)).collect()
}

/// Space between a leaf's item array and data, from raw bytes, so the setup
/// does not use the crate's arithmetic
fn leaf_free_bytes(d: &[u8]) -> u32 {
    let capacity = (d.len() - HEADER_SIZE) as u32;
    let n = u32_at(d, 96).unwrap();
    if n == 0 {
        return capacity;
    }
    let first_end = u32_at(d, HEADER_SIZE + 17).unwrap() + u32_at(d, HEADER_SIZE + 21).unwrap();
    let last_off = u32_at(d, HEADER_SIZE + ITEM_SIZE * (n as usize - 1) + 17).unwrap();
    capacity - (first_end - last_off) - ITEM_SIZE as u32 * n
}

/// Every leaf under a level-1 root, as `(blockptr, nritems, free space)`
fn leaf_row(t: &Trans<Scratch>, read: &mut dyn FnMut(u64) -> Result<Vec<u8>, BtrfsError>, root: u64)
    -> Vec<(u64, u32, u32)>
{
    let node = t.node_at(root, read).unwrap();
    assert_eq!(
        node.level, 1,
        "these cases want a root whose children are leaves and got a level-{} root",
        node.level
    );
    (0..node.nritems)
        .map(|slot| {
            let child = node.blockptr_at(slot).unwrap();
            let leaf = t.node_at(child, read).unwrap();
            (child, leaf.nritems, leaf_free_bytes(leaf.bytes()))
        })
        .collect()
}

/// A key strictly between two others, or `None` if adjacent. For inserting
/// mid-leaf: a synthetic key alone only appends, where nothing moves
fn key_between(a: &Key, b: &Key) -> Option<Key> {
    for c in [
        Key { offset: a.offset.checked_add(1)?, ..*a },
        Key { kind: a.kind.checked_add(1)?, offset: 0, ..*a },
        Key { objectid: a.objectid.checked_add(1)?, kind: 0, offset: 0 },
    ] {
        if *a < c && c < *b {
            return Some(c);
        }
    }
    None
}

/// xorshift64*, so a failure reproduces from the printed seed
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

// One item in, one item out

/// An insert places the item at its key, keeps the data tiled exactly, and
/// changes nothing else
#[test]
fn an_insert_lands_in_key_order_and_the_leaf_still_tiles() {
    let mut f = fixture("insert1");
    let sectorsize = f.sectorsize;
    let root = f.deep_root;
    let before = f.read_all(root);
    assert!(before.len() > 1000, "the fixture should be a large tree, got {}", before.len());

    let key = synth(1);
    let body = body_of(1, 64);

    let mut t = f.trans();
    let mut read = f.reader();
    t.insert_item(root, &key, &body, &mut read).expect("the insert must work");
    drop(read);

    check_every_dirty_block(&t, sectorsize);
    for (logical, data) in t.dirty_blocks() {
        assert_leaf_tiles(data, logical);
    }

    let new_root = t.new_root_of(root).expect("the root moved");
    let mut w = Walk::default();
    let mut read = f.reader();
    walk(&t, &mut read, new_root, None, 0, &mut w);
    drop(read);

    let mut want: Vec<(Key, Vec<u8>)> = before.clone();
    want.push((key, body.clone()));
    want.sort_by_key(|(k, _)| *k);
    assert_eq!(w.items.len(), before.len() + 1, "the tree gained more than one item");
    assert_eq!(w.items, want, "the tree does not hold exactly what it held plus the new item");
}

/// An item mid-leaf moves each item after it, in both arrays. The chunk tree's
/// leaf has room, so the shift is what is tested
#[test]
fn an_insert_between_two_items_moves_the_ones_after_it() {
    let mut f = fixture("middle");
    let sectorsize = f.sectorsize;
    let root = f.flat_root;
    let before = f.read_all(root);
    assert!(
        before.len() >= 3,
        "this case needs items on both sides of the insert and the chunk tree has {}",
        before.len()
    );

    let (at, key) = before
        .windows(2)
        .enumerate()
        .find_map(|(i, w)| {
            if i == 0 {
                return None;
            }
            key_between(&w[0].0, &w[1].0).map(|k| (i + 1, k))
        })
        .expect("no gap in the chunk tree's key space to insert into");
    assert!(at > 1 && at < before.len(), "the insert must have items on both sides");

    let body = body_of(7, 96);
    let mut t = f.trans();
    let mut read = f.reader();
    t.insert_item(root, &key, &body, &mut read).expect("the insert must work");
    drop(read);

    check_every_dirty_block(&t, sectorsize);
    for (logical, data) in t.dirty_blocks() {
        assert_leaf_tiles(data, logical);
    }

    let top = t.new_root_of(root).expect("the root moved");
    let mut read = f.reader();
    let leaf = t.node_at(top, &mut read).unwrap();
    assert!(leaf.is_leaf(), "this case is about one leaf and the tree grew a level");
    assert_eq!(leaf.nritems as usize, before.len() + 1);
    assert_eq!(leaf.key_at(at as u32).unwrap(), key, "the item did not land at slot {at}");
    assert_eq!(leaf.item_data(at as u32).unwrap(), &body[..], "the new item's bytes are wrong");

    let mut w = Walk::default();
    walk(&t, &mut read, top, None, 0, &mut w);
    drop(read);

    let mut want = before.clone();
    want.insert(at, (key, body));
    assert_eq!(w.items, want, "the items after the new one did not come through intact");
}

/// An item before everything lands at slot 0 of the leftmost leaf, and each
/// node above is told: `fixup_low_keys`. A stale smallest key sends a later
/// search into the prior subtree
#[test]
fn an_insert_before_everything_updates_the_keys_above_it() {
    let mut f = fixture("lowkey");
    let sectorsize = f.sectorsize;
    let root = f.deep_root;
    let before = f.read_all(root);
    let first = before[0].0;

    // Strictly smaller than anything held. No valid key type is zero, so this
    // is below each real key with the objectid
    let key = if first.kind > 0 {
        Key { objectid: first.objectid, kind: 0, offset: 0 }
    } else {
        Key { objectid: first.objectid - 1, kind: 0, offset: 0 }
    };
    assert!(key < first, "the key for this case is not below the tree's first key");

    let mut t = f.trans();
    let mut read = f.reader();
    t.insert_item(root, &key, &body_of(11, 48), &mut read).expect("the insert must work");

    let top = t.new_root_of(root).expect("the root moved");
    let node = t.node_at(top, &mut read).unwrap();
    assert!(!node.is_leaf(), "this case wants a tree with a parent to correct");
    assert_eq!(
        node.key_at(0).unwrap(),
        key,
        "the root still carries the key that used to be the smallest in the tree"
    );
    let leftmost = t.node_at(node.blockptr_at(0).unwrap(), &mut read).unwrap();
    assert_eq!(leftmost.key_at(0).unwrap(), key, "the item did not land at slot 0");

    check_every_dirty_block(&t, sectorsize);
    let mut w = Walk::default();
    walk(&t, &mut read, top, None, 0, &mut w);
    drop(read);

    let mut want = before;
    want.insert(0, (key, body_of(11, 48)));
    assert_eq!(w.items, want, "the tree does not hold what it held plus the new first item");
}

/// A delete closes the gap in both arrays. A hole left in the data region makes
/// the kernel refuse the whole block
#[test]
fn a_delete_closes_the_gap_and_the_leaf_still_tiles() {
    let mut f = fixture("delete1");
    let sectorsize = f.sectorsize;
    let root = f.deep_root;
    let before = f.read_all(root);

    // From the middle, so the item has neighbours on both sides and the data of
    // later items moves
    let victim = before[before.len() / 2].0;

    let mut t = f.trans();
    let mut read = f.reader();
    t.delete_item(root, &victim, &mut read).expect("the delete must work");
    drop(read);

    check_every_dirty_block(&t, sectorsize);
    for (logical, data) in t.dirty_blocks() {
        assert_leaf_tiles(data, logical);
    }

    let new_root = t.new_root_of(root).expect("the root moved");
    let mut w = Walk::default();
    let mut read = f.reader();
    walk(&t, &mut read, new_root, None, 0, &mut w);
    drop(read);

    let want: Vec<(Key, Vec<u8>)> =
        before.iter().filter(|(k, _)| *k != victim).cloned().collect();
    assert_eq!(w.items.len(), before.len() - 1, "the tree lost more than one item");
    assert_eq!(w.items, want, "the tree does not hold exactly what it held minus one item");
}

/// A missing key is `NotFound`, and nothing is copied
#[test]
fn deleting_a_key_that_is_not_there_changes_nothing() {
    let mut f = fixture("absent");
    let root = f.deep_root;

    let mut t = f.trans();
    let mut read = f.reader();
    let r = t.delete_item(root, &synth(9), &mut read);
    drop(read);

    assert!(matches!(r, Err(BtrfsError::NotFound)), "got {r:?}");
    assert_eq!(t.dirty_count(), 0, "a refused delete copied blocks anyway");
    assert!(t.new_root_of(root).is_none(), "a refused delete moved the root");
}

/// An existing key is refused, and nothing is copied. Two items with one key
/// break every binary search in this crate
#[test]
fn inserting_a_key_that_is_already_there_changes_nothing() {
    let mut f = fixture("dup");
    let root = f.deep_root;
    let existing = f.read_all(root)[0].0;

    let mut t = f.trans();
    let mut read = f.reader();
    let r = t.insert_item(root, &existing, &[0u8; 8], &mut read);
    drop(read);

    assert!(matches!(r, Err(BtrfsError::InvalidArg)), "got {r:?}");
    assert_eq!(t.dirty_count(), 0, "a refused insert copied blocks anyway");
}

/// An item too big for an empty leaf is refused
#[test]
fn an_item_bigger_than_a_leaf_is_refused() {
    let mut f = fixture("toobig");
    let nodesize = f.nodesize;
    let root = f.deep_root;
    let capacity = nodesize as usize - HEADER_SIZE;

    let mut t = f.trans();
    let mut read = f.reader();
    let just_too_big = vec![0u8; capacity - ITEM_SIZE + 1];
    let r = t.insert_item(root, &synth(2), &just_too_big, &mut read);
    let fits = t.insert_item(root, &synth(3), &vec![0u8; capacity - ITEM_SIZE], &mut read);
    drop(read);

    assert!(matches!(r, Err(BtrfsError::InvalidArg)), "got {r:?}");
    fits.expect("an item of exactly the largest size that fits must be accepted");
}

// Relieving a leaf into a sibling still on the medium
//
// The sibling's address has to be copied before it is taken from the parent. A
// `mkfs -r` volume is the fixture where a leaf's neighbour is certainly
// untouched, and a leaf packed to a few dozen bytes beside one with kilobytes
// free is what triggers the relief.

/// An insert into a full leaf pushes items into its right sibling, and the
/// sibling is copied first
///
/// `push_leaf_right`. The sibling is untouched by this transaction, the one
/// state where a missing copy shows
#[test]
fn an_insert_relieves_a_full_leaf_into_its_untouched_right_sibling() {
    let mut f = fixture("pushright");
    let sectorsize = f.sectorsize;
    let root = f.deep_root;
    let before = f.read_all(root);

    let t0 = f.trans();
    let mut read = f.reader();
    let row = leaf_row(&t0, &mut read, root);
    drop(read);

    // A leaf with almost nothing left, beside one with room, and with a gap in
    // its own key space to insert into. btrfs keys are often consecutive, so
    // not every packed leaf has one
    let (at, key, slot_in_leaf) = {
        let t = f.trans();
        let mut read = f.reader();
        let found = (0..row.len() - 1)
            .filter(|&i| row[i].2 < 2000 && row[i].1 >= 8 && row[i + 1].2 > 4000)
            .find_map(|i| {
                let leaf = t.node_at(row[i].0, &mut read).unwrap();
                // Inside the leaf, not at its end: the items at and after the
                // slot get pushed right
                (0..leaf.nritems - 1).find_map(|j| {
                    key_between(&leaf.key_at(j).unwrap(), &leaf.key_at(j + 1).unwrap())
                        .map(|k| (i, k, j + 1))
                })
            });
        drop(read);
        found.expect(
            "no packed leaf with a roomy right neighbour and a gap to insert into - \
             without one this case proves nothing, so it fails rather than passing quietly",
        )
    };
    assert!(slot_in_leaf < row[at].1, "the key must land inside the leaf, not past its end");

    // Too big for the leaf, small enough for the right sibling to take what
    // comes across
    let body = body_of(3, row[at].2 as usize);

    let mut t = f.trans();
    let mut read = f.reader();
    t.insert_item(root, &key, &body, &mut read).expect("the insert must work");

    let top = t.new_root_of(root).expect("the root moved");
    let after = leaf_row(&t, &mut read, top);
    assert_eq!(
        after.len(),
        row.len(),
        "the leaf split instead of being relieved, so this case did not test the push"
    );
    assert!(
        after[at].1 < row[at].1 + 1,
        "leaf {at} went from {} items to {} - nothing was pushed out of it",
        row[at].1,
        after[at].1
    );
    assert!(
        after[at + 1].1 > row[at + 1].1,
        "the right sibling went from {} items to {} - nothing arrived",
        row[at + 1].1,
        after[at + 1].1
    );
    assert!(t.is_dirty(after[at + 1].0), "the sibling was written to and is not ours");

    check_every_dirty_block(&t, sectorsize);
    let mut w = Walk::default();
    walk(&t, &mut read, top, None, 0, &mut w);
    drop(read);

    let mut want = before;
    want.push((key, body));
    want.sort_by_key(|(k, _)| *k);
    assert_eq!(w.items, want, "the pushed items did not come through intact");
}

/// The same, to the left sibling
///
/// The key goes past everything in the leaf, so `push_leaf_right` has nothing
/// to move
#[test]
fn an_insert_relieves_a_full_leaf_into_its_untouched_left_sibling() {
    let mut f = fixture("pushleft");
    let sectorsize = f.sectorsize;
    let root = f.deep_root;
    let before = f.read_all(root);

    let t0 = f.trans();
    let mut read = f.reader();
    let row = leaf_row(&t0, &mut read, root);
    drop(read);

    // Past the end of this leaf and before the start of the next. The slot is
    // the leaf's end, which leaves only the left sibling
    let (at, key) = {
        let t = f.trans();
        let mut read = f.reader();
        let found = (1..row.len() - 1)
            .filter(|&i| row[i - 1].2 > 4000 && row[i].2 < 2000 && row[i].1 >= 8)
            .find_map(|i| {
                let leaf = t.node_at(row[i].0, &mut read).unwrap();
                let next = t.node_at(row[i + 1].0, &mut read).unwrap();
                key_between(&leaf.key_at(leaf.nritems - 1).unwrap(), &next.key_at(0).unwrap())
                    .map(|k| (i, k))
            });
        drop(read);
        found.expect(
            "no packed leaf with a roomy left neighbour and a gap past its last key - \
             without one this case proves nothing, so it fails rather than passing quietly",
        )
    };
    let body = body_of(5, row[at].2 as usize);

    let mut t = f.trans();
    let mut read = f.reader();
    t.insert_item(root, &key, &body, &mut read).expect("the insert must work");

    let top = t.new_root_of(root).expect("the root moved");
    let after = leaf_row(&t, &mut read, top);
    assert_eq!(
        after.len(),
        row.len(),
        "the leaf split instead of being relieved, so this case did not test the push"
    );
    assert!(
        after[at - 1].1 > row[at - 1].1,
        "the left sibling went from {} items to {} - nothing arrived",
        row[at - 1].1,
        after[at - 1].1
    );
    assert!(t.is_dirty(after[at - 1].0), "the sibling was written to and is not ours");

    check_every_dirty_block(&t, sectorsize);
    let mut w = Walk::default();
    walk(&t, &mut read, top, None, 0, &mut w);
    drop(read);

    let mut want = before;
    want.push((key, body));
    want.sort_by_key(|(k, _)| *k);
    assert_eq!(w.items, want, "the pushed items did not come through intact");
}

// Splitting

/// Fill a leaf until it splits, and check both halves and the parent
///
/// On a volume this size the chunk tree is a single leaf, so this covers a tree
/// growing its first level and a leaf splitting into a new parent
#[test]
fn a_full_leaf_splits_and_the_parent_points_at_both_halves() {
    let mut f = fixture("split");
    let (nodesize, sectorsize) =
        (f.nodesize, f.sectorsize);
    let root = f.flat_root;
    let before = f.read_all(root);
    let capacity = nodesize - HEADER_SIZE as u32;

    // An ordinary split moves half the leaf into a new sibling and takes the
    // parent's key from the sibling's first item. It happens only when half a
    // leaf plus the new item fits in a leaf. With items a quarter of a leaf
    // each, every split gives the new item its own leaf and the split key is
    // never computed, hence a twentieth
    let body_len = (capacity / 20) as usize;
    let per_leaf = capacity / (body_len as u32 + ITEM_SIZE as u32);
    assert!(per_leaf >= 15 && per_leaf <= 25, "sizing is wrong: {per_leaf} items per leaf");

    let mut t = f.trans();
    let mut read = f.reader();
    let mut model: BTreeMap<Key, Vec<u8>> = before.iter().cloned().collect();
    let mut split_at = None;
    for i in 0..(per_leaf as u64 + 2) {
        let (k, b) = (synth(i), body_of(i, body_len));
        t.insert_item(root, &k, &b, &mut read).expect("the insert must work");
        model.insert(k, b);
        let top = t.new_root_of(root).expect("the root moved");
        let node = t.node_at(top, &mut read).unwrap();
        if node.level > 0 && split_at.is_none() {
            split_at = Some(i);
        }
    }
    drop(read);

    let i = split_at.expect("the leaf never split - the sizing above is wrong, not the code");
    assert!(i < per_leaf as u64 + 2);

    let top = t.new_root_of(root).unwrap();
    let mut read = f.reader();
    let node = t.node_at(top, &mut read).unwrap();
    assert_eq!(node.level, 1, "the tree should have grown exactly one level");
    assert!(node.nritems >= 2, "a split gives the new root two children, got {}", node.nritems);

    // Both halves are sound leaves, the parent's key for each is that half's
    // first key, and the halves do not overlap
    let mut prev_last: Option<Key> = None;
    let mut sizeable = 0usize;
    for slot in 0..node.nritems {
        let child = node.blockptr_at(slot).unwrap();
        let leaf = t.node_at(child, &mut read).unwrap();
        assert!(leaf.is_leaf(), "a level-1 node's children are leaves");
        assert!(leaf.nritems > 0, "a split must not leave an empty half");
        assert_eq!(
            leaf.key_at(0).unwrap(),
            node.key_at(slot).unwrap(),
            "the parent's key at slot {slot} is not that child's first key - which is what \
             a split key taken from the wrong side of the split looks like"
        );
        if let Some(p) = prev_last {
            assert!(p < leaf.key_at(0).unwrap(), "the two halves overlap");
        }
        if leaf.nritems >= 2 {
            sizeable += 1;
        }
        prev_last = Some(leaf.key_at(leaf.nritems - 1).unwrap());
    }
    assert!(
        sizeable >= 2,
        "every leaf but one holds a single item, which means the leaf was never split in \
         half - it was emptied one item at a time into leaves of their own, and the split \
         key this case is about was never worked out"
    );

    check_every_dirty_block(&t, sectorsize);
    let mut w = Walk::default();
    walk(&t, &mut read, top, None, 0, &mut w);
    drop(read);
    let got: BTreeMap<Key, Vec<u8>> = w.items.into_iter().collect();
    assert_eq!(got, model, "the split tree does not hold what was put into it");
}

/// Keep going until the root node fills and splits too
///
/// A level-1 root holds `(nodesize - 101) / 33` pointers and splits three short
/// of that. Two items per leaf reach a third level in a few hundred inserts.
/// `split_node` and the second `insert_new_root` run only there
#[test]
fn a_full_root_node_splits_and_the_tree_gains_a_third_level() {
    let mut f = fixture("nodesplit");
    let (nodesize, sectorsize) =
        (f.nodesize, f.sectorsize);
    let root = f.flat_root;
    let before = f.read_all(root);
    let capacity = nodesize - HEADER_SIZE as u32;
    let ptrs_per_node = (nodesize - HEADER_SIZE as u32) / KEY_PTR_SIZE as u32;

    // Two items to a leaf: a new leaf every other insert
    let body_len = (capacity / 2 - ITEM_SIZE as u32 - 8) as usize;
    let mut model: BTreeMap<Key, Vec<u8>> = before.iter().cloned().collect();

    let mut t = f.trans();
    let mut read = f.reader();
    // More leaves than a node holds pointers, with room for leaves that end up
    // holding one item
    let count = (ptrs_per_node as u64 + 8) * 2;
    for i in 0..count {
        let (k, b) = (synth(i), body_of(i, body_len));
        t.insert_item(root, &k, &b, &mut read).expect("the insert must work");
        model.insert(k, b);
    }

    let top = t.new_root_of(root).expect("the root moved");
    let node = t.node_at(top, &mut read).unwrap();
    assert!(
        node.level >= 2,
        "the root is still level {} after {count} inserts - this case is meant to force a \
         node split and did not",
        node.level
    );

    check_every_dirty_block(&t, sectorsize);
    let mut w = Walk::default();
    walk(&t, &mut read, top, None, 0, &mut w);
    drop(read);
    assert!(w.leaves > ptrs_per_node as usize, "expected more leaves than a node holds");
    let got: BTreeMap<Key, Vec<u8>> = w.items.into_iter().collect();
    assert_eq!(got, model, "the tree does not hold what was put into it");
}

/// Everything inserted comes back out, and the tree gives back its levels on
/// the way
///
/// A split allocates blocks and a delete frees them, so this runs both against
/// one tree
#[test]
fn deleting_everything_gives_the_levels_back() {
    let mut f = fixture("collapse");
    let (nodesize, sectorsize) =
        (f.nodesize, f.sectorsize);
    let root = f.flat_root;
    let before = f.read_all(root);
    let capacity = nodesize - HEADER_SIZE as u32;
    let body_len = (capacity / 4) as usize;
    let count = 40u64;

    let mut t = f.trans();
    let mut read = f.reader();
    for i in 0..count {
        t.insert_item(root, &synth(i), &body_of(i, body_len), &mut read).unwrap();
    }
    let grown = t.new_root_of(root).unwrap();
    let grown_level = t.node_at(grown, &mut read).unwrap().level;
    assert!(grown_level >= 1, "the inserts did not grow the tree, so the shrink proves nothing");

    for i in 0..count {
        t.delete_item(root, &synth(i), &mut read).unwrap();
        check_every_dirty_block(&t, sectorsize);
        let top = t.new_root_of(root).unwrap();
        let mut w = Walk::default();
        walk(&t, &mut read, top, None, 0, &mut w);
        let left = count - i - 1;
        assert_eq!(
            w.items.len(),
            before.len() + left as usize,
            "after deleting {} of {count} the tree holds the wrong number of items",
            i + 1
        );
    }

    let top = t.new_root_of(root).unwrap();
    let level_now = t.node_at(top, &mut read).unwrap().level;
    let mut w = Walk::default();
    walk(&t, &mut read, top, None, 0, &mut w);
    drop(read);

    assert_eq!(
        level_now, 0,
        "everything that was added has been taken away and the tree is still {level_now} \
         levels deep - a root with one child is a read nobody is getting anything for"
    );
    assert_eq!(w.items, before, "the tree did not come back to what it started as");
}

/// Build a three-level tree and take it apart
///
/// An internal node loses its last pointer only in a level-2 tree, when a
/// deletion empties every leaf beneath one node. `drop_ptr` walks up through it
/// and frees the block after the pointer to it is gone
#[test]
fn a_three_level_tree_comes_apart_again() {
    let mut f = fixture("teardown");
    let (nodesize, sectorsize) = (f.nodesize, f.sectorsize);
    let root = f.flat_root;
    let before = f.read_all(root);
    let capacity = nodesize - HEADER_SIZE as u32;
    let ptrs_per_node = capacity / KEY_PTR_SIZE as u32;
    let body_len = (capacity / 2 - ITEM_SIZE as u32 - 8) as usize;
    let count = (ptrs_per_node as u64 + 8) * 2;

    let mut t = f.trans();
    let mut read = f.reader();
    for i in 0..count {
        t.insert_item(root, &synth(i), &body_of(i, body_len), &mut read).unwrap();
    }
    let built = t.node_at(t.new_root_of(root).unwrap(), &mut read).unwrap().level;
    assert!(built >= 2, "the build phase gave a level-{built} tree, so the teardown proves nothing");

    // Delete from the middle outward, so whole subtrees empty
    let mut order: Vec<u64> = (0..count).collect();
    order.sort_by_key(|i| (*i as i64 - count as i64 / 2).abs());
    for (n, i) in order.iter().enumerate() {
        t.delete_item(root, &synth(*i), &mut read).unwrap();
        // A walk every sixteenth step keeps this case to seconds. The
        // randomised case checks every step
        if n % 16 == 0 || n + 1 == order.len() {
            check_every_dirty_block(&t, sectorsize);
            let top = t.new_root_of(root).unwrap();
            let mut w = Walk::default();
            walk(&t, &mut read, top, None, 0, &mut w);
            assert_eq!(
                w.items.len(),
                before.len() + count as usize - n - 1,
                "after {} deletions the tree holds the wrong number of items",
                n + 1
            );
        }
    }

    let top = t.new_root_of(root).unwrap();
    let level_now = t.node_at(top, &mut read).unwrap().level;
    let mut w = Walk::default();
    walk(&t, &mut read, top, None, 0, &mut w);
    drop(read);

    assert_eq!(level_now, 0, "a tree with {} items left is still level {level_now}", w.items.len());
    assert_eq!(w.items, before, "the tree did not come back to what it started as");
}

// The randomised sequence

/// A few hundred inserts and deletes against a `BTreeMap`, walking the whole
/// tree after every step
///
/// The seed is fixed so a failure reproduces; the step number and the seed are
/// in every message. A merge and a split can produce corruption that is gone
/// again by the end, so the check runs per step
#[test]
fn a_random_sequence_of_inserts_and_deletes_matches_a_model() {
    const SEED: u64 = 0x5EED_1234_ABCD_0001;
    const STEPS: usize = 400;

    let mut f = fixture("random");
    let (nodesize, sectorsize) =
        (f.nodesize, f.sectorsize);
    let root = f.flat_root;
    let before = f.read_all(root);
    let mut model: BTreeMap<Key, Vec<u8>> = before.iter().cloned().collect();

    let mut rng = Rng(SEED);
    let mut t = f.trans();
    let mut read = f.reader();
    let mut live: Vec<Key> = Vec::new();
    let mut inserts = 0usize;
    let mut deletes = 0usize;
    let mut max_level = 0u8;
    let mut max_leaves = 0usize;
    let mut min_leaves = usize::MAX;

    for step in 0..STEPS {
        // Biased toward inserting, so the tree grows to several levels before
        // the deletes take it apart
        let insert = live.is_empty() || rng.below(100) < 60;
        if insert {
            let n = rng.below(100_000);
            let k = synth(n);
            if model.contains_key(&k) {
                // Already there: the insert refuses it and changes nothing
                let r = t.insert_item(root, &k, &[0u8; 4], &mut read);
                assert!(
                    matches!(r, Err(BtrfsError::InvalidArg)),
                    "step {step} (seed {SEED:#x}): a duplicate key was not refused: {r:?}"
                );
            } else {
                // Sizes across the whole range, so leaves fill at different
                // rates and both kinds of split get reached
                let len = match rng.below(10) {
                    0 => (nodesize as usize - HEADER_SIZE) / 2 + 40,
                    1..=3 => rng.below(900) as usize + 1,
                    _ => rng.below(80) as usize + 1,
                };
                let b = body_of(n, len);
                t.insert_item(root, &k, &b, &mut read).unwrap_or_else(|e| {
                    panic!("step {step} (seed {SEED:#x}): insert of {len} bytes failed: {e:?}")
                });
                model.insert(k, b);
                live.push(k);
                inserts += 1;
            }
        } else {
            let at = rng.below(live.len() as u64) as usize;
            let k = live.swap_remove(at);
            t.delete_item(root, &k, &mut read).unwrap_or_else(|e| {
                panic!("step {step} (seed {SEED:#x}): delete of {k:?} failed: {e:?}")
            });
            model.remove(&k);
            deletes += 1;
        }

        check_every_dirty_block(&t, sectorsize);
        let top = t.new_root_of(root).unwrap_or(root);
        let mut w = Walk::default();
        walk(&t, &mut read, top, None, 0, &mut w);
        max_level = max_level.max(w.max_level);
        max_leaves = max_leaves.max(w.leaves);
        min_leaves = min_leaves.min(w.leaves);

        let got: BTreeMap<Key, Vec<u8>> = w.items.into_iter().collect();
        assert_eq!(
            got.len(),
            model.len(),
            "step {step} (seed {SEED:#x}): the tree holds {} items and the model {}",
            got.len(),
            model.len()
        );
        assert!(
            got == model,
            "step {step} (seed {SEED:#x}): the tree and the model disagree"
        );
    }
    drop(read);

    // The sequence has to have split and merged for the assertions above to
    // mean anything
    assert!(inserts > 100, "only {inserts} inserts happened");
    assert!(deletes > 50, "only {deletes} deletes happened");
    assert!(max_level >= 1, "the tree never grew past one leaf, so nothing ever split");
    assert!(max_leaves >= 8, "the tree never got past {max_leaves} leaves");
    assert!(
        min_leaves < max_leaves,
        "the tree never lost a leaf, so no merge was ever exercised"
    );
}

// The three copy-on-write rules, through splits and merges

/// Rule 1: a block the transaction copied away from belongs to the old tree,
/// which a crash falls back to
///
/// A split allocates and a merge frees, and the two together can hand a
/// just-freed address back out
#[test]
fn a_pinned_block_is_never_handed_out_again_through_splits_and_merges() {
    let mut f = fixture("rule1");
    let nodesize = f.nodesize;
    let root = f.deep_root;
    let items = f.read_all(root);
    let capacity = nodesize - HEADER_SIZE as u32;

    let mut t = f.trans();
    let mut read = f.reader();
    // Inserts big enough to split the packed leaves of a `mkfs -r` volume, then
    // enough deletes to empty leaves and merge
    for i in 0..40u64 {
        t.insert_item(root, &synth(i), &body_of(i, (capacity / 4) as usize), &mut read)
            .unwrap();
    }
    for (k, _) in items.iter().take(400) {
        t.delete_item(root, k, &mut read).unwrap();
    }
    drop(read);

    let pinned: HashSet<u64> = t.freed_blocks().collect();
    assert!(!pinned.is_empty(), "copying a path pins the block it copied");

    let handed: HashSet<u64> = t.alloc.handed_out.iter().copied().collect();
    let overlap: Vec<u64> = pinned.intersection(&handed).copied().collect();
    assert!(
        overlap.is_empty(),
        "these blocks were pinned and handed out again in the same transaction: {overlap:?}"
    );

    for (logical, _) in t.dirty_blocks() {
        assert!(
            !pinned.contains(&logical),
            "block {logical} is pinned and is also a block this transaction will write"
        );
    }
}

/// An allocator that hands back a pinned address is refused
#[test]
fn an_allocator_that_hands_back_a_pinned_block_is_refused() {
    let mut f = fixture("rule1refuse");
    let root = f.deep_root;
    let key = f.read_all(root)[0].0;

    let mut t = f.trans();
    let mut read = f.reader();
    // One insert, to get a pinned address to hand back
    t.insert_item(root, &synth(1), &[0u8; 16], &mut read).unwrap();
    let pinned = t.freed_blocks().next().expect("something was pinned");

    t.alloc.hand_back_pinned = Some(pinned);
    let r = t.delete_item(root, &key, &mut read);
    drop(read);

    assert!(
        r.is_err(),
        "an allocator handed back a block the old tree still points at and it was accepted"
    );
}

/// Rule 2: one block, one copy. A second copy leaks the first and leaves a
/// stale pointer, and a split touches three blocks at once
#[test]
fn splitting_copies_each_block_at_most_once() {
    let mut f = fixture("rule2");
    let nodesize = f.nodesize;
    let root = f.flat_root;
    let capacity = nodesize - HEADER_SIZE as u32;

    let mut t = f.trans();
    let mut read = f.reader();
    for i in 0..30u64 {
        t.insert_item(root, &synth(i), &body_of(i, (capacity / 4) as usize), &mut read)
            .unwrap();
    }
    let top = t.new_root_of(root).unwrap();

    // Every block the transaction will write is reachable from the new root
    // exactly once. A block copied twice shows as dirty and unreachable, and
    // the reachable count comes up short
    let mut w = Walk::default();
    walk(&t, &mut read, top, None, 0, &mut w);
    drop(read);

    let mut seen: HashSet<u64> = HashSet::new();
    let mut stack = vec![top];
    let mut reachable = 0usize;
    let mut read = f.reader();
    while let Some(l) = stack.pop() {
        if !seen.insert(l) {
            panic!("block {l} is reachable from the new root twice");
        }
        if t.is_dirty(l) {
            reachable += 1;
        }
        let node = t.node_at(l, &mut read).unwrap();
        if !node.is_leaf() {
            for slot in 0..node.nritems {
                stack.push(node.blockptr_at(slot).unwrap());
            }
        }
    }
    drop(read);

    let dirty: HashSet<u64> = t.dirty_blocks().map(|(l, _)| l).collect();
    assert_eq!(
        reachable,
        dirty.len(),
        "{} of this transaction's blocks are not reachable from its root - a block copied \
         twice leaves the first copy exactly like this",
        dirty.len() - reachable
    );
}

/// Rule 3: a copied parent points at the copied child. In a split the parent is
/// the copy and the new half is this transaction's block
#[test]
fn every_block_of_a_split_tree_belongs_to_the_transaction() {
    let mut f = fixture("rule3");
    let nodesize = f.nodesize;
    let root = f.deep_root;
    let capacity = nodesize - HEADER_SIZE as u32;

    let mut t = f.trans();
    let mut read = f.reader();
    for i in 0..20u64 {
        t.insert_item(root, &synth(i), &body_of(i, (capacity / 4) as usize), &mut read)
            .unwrap();
    }
    let top = t.new_root_of(root).unwrap();

    // Walk to every leaf holding a synthetic key. Every block on the way
    // belongs to this transaction: a parent pointing at the old child reads
    // correctly until the old block is reused
    for i in 0..20u64 {
        let k = synth(i);
        let mut logical = top;
        loop {
            assert!(
                t.is_dirty(logical),
                "block {logical}, on the path to {k:?}, is still the medium's copy"
            );
            let node = t.node_at(logical, &mut read).unwrap();
            if node.is_leaf() {
                assert!(node.search(&k).unwrap().is_ok(), "{k:?} is not in the leaf it leads to");
                break;
            }
            let slot = match node.search(&k).unwrap() {
                Ok(e) => e,
                Err(0) => panic!("{k:?} sorts before the whole tree"),
                Err(n) => n - 1,
            };
            logical = node.blockptr_at(slot).unwrap();
        }
    }
    drop(read);
}

// What a mount makes of it

/// Sealed blocks go through the parser a mount uses (checksum, fsid, own
/// address, `WRITTEN` flag, structural rules), and the sealed tree holds what
/// the in-flight one did
#[test]
fn a_sealed_reshaped_tree_satisfies_the_ordinary_reader() {
    let mut f = fixture("seal");
    let (nodesize, sectorsize, fsid, generation) =
        (f.nodesize, f.sectorsize, f.fsid, f.generation);
    let root = f.deep_root;
    let before = f.read_all(root);
    let capacity = nodesize - HEADER_SIZE as u32;

    let mut t = f.trans();
    let mut read = f.reader();
    for i in 0..12u64 {
        t.insert_item(root, &synth(i), &body_of(i, (capacity / 4) as usize), &mut read)
            .unwrap();
    }
    for (k, _) in before.iter().take(30) {
        t.delete_item(root, k, &mut read).unwrap();
    }
    drop(read);

    let new_root = t.new_root_of(root).unwrap();
    let generation = generation + 1;
    let sealed: HashMap<u64, Vec<u8>> = t.seal().unwrap().into_iter().collect();
    assert!(!sealed.is_empty());

    for (logical, data) in &sealed {
        let node = parse_node(data.clone(), *logical, &fsid, sectorsize)
            .unwrap_or_else(|e| panic!("block {logical} would be refused on a mount: {e:?}"));
        assert_eq!(node.bytenr, *logical);
        assert_eq!(node.generation, generation, "a block of this transaction carries its generation");
        assert_eq!(u32_at(data, 0).unwrap(), fshost::crc32c::crc32c(0, &data[CSUM_SIZE..]));
        assert_leaf_tiles(data, *logical);
    }

    // And the sealed tree holds what it should
    let mut want: BTreeMap<Key, Vec<u8>> = before.iter().cloned().collect();
    for (k, _) in before.iter().take(30) {
        want.remove(k);
    }
    for i in 0..12u64 {
        want.insert(synth(i), body_of(i, (capacity / 4) as usize));
    }

    let mut got: Vec<(Key, Vec<u8>)> = Vec::new();
    let mut medium = f.reader();
    walk_sealed(&sealed, &mut medium, new_root, &fsid, sectorsize, &mut got, 0);
    drop(medium);
    let got: BTreeMap<Key, Vec<u8>> = got.into_iter().collect();
    assert_eq!(got, want, "the sealed tree does not hold what the transaction built");
}

fn walk_sealed(
    sealed: &HashMap<u64, Vec<u8>>,
    medium: &mut dyn FnMut(u64) -> Result<Vec<u8>, BtrfsError>,
    logical: u64,
    fsid: &[u8; FSID_SIZE],
    sectorsize: u32,
    out: &mut Vec<(Key, Vec<u8>)>,
    depth: usize,
) {
    assert!(depth < MAX_LEVEL as usize + 1, "a tree that deep is a loop");
    let data = match sealed.get(&logical) {
        Some(d) => d.clone(),
        None => medium(logical).expect("the medium must still hold the untouched blocks"),
    };
    let node = parse_node(data, logical, fsid, sectorsize)
        .unwrap_or_else(|e| panic!("block {logical} would be refused on a mount: {e:?}"));
    if node.is_leaf() {
        for slot in 0..node.nritems {
            out.push((node.key_at(slot).unwrap(), node.item_data(slot).unwrap().to_vec()));
        }
        return;
    }
    for slot in 0..node.nritems {
        walk_sealed(
            sealed,
            medium,
            node.blockptr_at(slot).unwrap(),
            fsid,
            sectorsize,
            out,
            depth + 1,
        );
    }
}

/// Nothing above wrote anything. The old tree stays intact until a superblock
/// says otherwise
#[test]
fn reshaping_a_tree_writes_nothing() {
    let mut f = fixture("nowrite");
    let nodesize = f.nodesize;
    let root = f.deep_root;
    let items = f.read_all(root);
    let before = f.vol.host_mut().data.clone();
    let capacity = nodesize - HEADER_SIZE as u32;

    let mut t = f.trans();
    let mut read = f.reader();
    for i in 0..20u64 {
        t.insert_item(root, &synth(i), &body_of(i, (capacity / 3) as usize), &mut read)
            .unwrap();
    }
    for (k, _) in items.iter().take(200) {
        t.delete_item(root, k, &mut read).unwrap();
    }
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
