// The extent tree, judged externally
//
// `src/extent.rs` writes the item saying a tree block exists and who owns it. A
// wrong refcount, a level for a length, or a backref to the wrong tree read
// back here as written.
//
//   * `mkfs.btrfs` as the oracle for bytes. Each tree block's level, owner, and
//     generation can be read, so this
//     re-encodes each item from them and requires equality with what `mkfs` wrote, covering the key's offset,
//     refcount, flags, backref type, and backref root.
//   * `btrfs check` as the judge of the volume. `[3/8] checking extents`
//     cross-checks extent items against
//     existing tree blocks and the backrefs claiming them.
//
// Nothing here skips. `require_tools` below fails without btrfs-progs, and
// fixtures panic instead of backing out

mod common;

use btrfs::commit::{commit_superblocks, write_block};
use btrfs::error::BtrfsError;
use btrfs::extent::*;
use btrfs::fs_tree::{root_item, Root, ROOT_ITEM_KEY};
use btrfs::node::header;
use btrfs::space::{
    allocator_for, read_free_space, settle as space_settle, SpaceInfo,
    BLOCK_GROUP_TREE_OBJECTID, EXTENT_TREE_OBJECTID, FREE_SPACE_TREE_OBJECTID,
};
use btrfs::structs::*;
use btrfs::trans::{BlockAlloc, Trans};
use btrfs::Volume;
use common::Image;
use fshost::{BlockHost, HostError};
use std::collections::BTreeMap;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

// A host writing through to the image file, so `btrfs check` sees what the
// driver did

struct FileHost {
    file: std::fs::File,
    sectors: u64,
}

impl FileHost {
    fn open(path: &Path) -> Self {
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .expect("cannot open the image for writing");
        let sectors = file.metadata().expect("cannot stat the image").len() / 512;
        FileHost { file, sectors }
    }

    fn seek_to(&mut self, lba: u64, count: u32) -> Result<usize, HostError> {
        let end = lba.checked_add(count as u64).ok_or(HostError::Io)?;
        if end > self.sectors {
            return Err(HostError::Io);
        }
        self.file.seek(SeekFrom::Start(lba * 512)).map_err(|_| HostError::Io)?;
        Ok(count as usize * 512)
    }
}

impl BlockHost for FileHost {
    fn read_sectors(&mut self, lba: u64, count: u32, buf: &mut [u8]) -> Result<(), HostError> {
        let n = self.seek_to(lba, count)?.min(buf.len());
        self.file.read_exact(&mut buf[..n]).map_err(|_| HostError::Io)
    }
    fn write_sectors(&mut self, lba: u64, count: u32, buf: &[u8]) -> Result<(), HostError> {
        let n = self.seek_to(lba, count)?;
        self.file.write_all(&buf[..n]).map_err(|_| HostError::Io)
    }
    fn write_sectors_sync(&mut self, lba: u64, count: u32, buf: &[u8]) -> Result<(), HostError> {
        self.write_sectors(lba, count, buf)
    }
    fn write_sectors_barrier(&mut self, lba: u64, count: u32, buf: &[u8]) -> Result<(), HostError> {
        self.write_sectors(lba, count, buf)
    }
    fn flush(&mut self) -> Result<(), HostError> {
        self.file.sync_data().map_err(|_| HostError::Io)
    }
    fn total_sectors(&self) -> u64 {
        self.sectors
    }
}

// An allocator over the volume's real free space
//
// `tests/shape.rs` uses a scratch range at 1 TiB, which no checker can read.
// Here addresses are real: inside a metadata block group, nodesize aligned,
// from space the free space tree calls free.
//
// Not this tier's allocator. It reads the free space tree once and does not
// write it, and the `btrfs check` output at the end shows that absence

struct ChunkAlloc {
    /// Free runs in metadata block groups, nodesize aligned, used front to back
    runs: Vec<(u64, u64)>,
    nodesize: u64,
    handed_out: Vec<u64>,
    /// Addresses no longer referenced. Recorded, not returned to `runs`: rule 1
    /// in `trans.rs`
    freed: Vec<u64>,
}

impl ChunkAlloc {
    fn over(space: &SpaceInfo, nodesize: u64) -> Self {
        let mut runs: Vec<(u64, u64)> = Vec::new();
        for g in &space.groups {
            if !g.is_metadata() {
                continue;
            }
            for &(start, len) in &g.runs {
                let aligned = start.div_ceil(nodesize) * nodesize;
                let end = start + len;
                if end > aligned && end - aligned >= nodesize {
                    runs.push((aligned, end - aligned));
                }
            }
        }
        assert!(
            !runs.is_empty(),
            "no free metadata space on this volume, so nothing below can allocate a \
             real address and nothing below can be judged from outside"
        );
        ChunkAlloc { runs, nodesize, handed_out: Vec::new(), freed: Vec::new() }
    }
}

impl BlockAlloc for ChunkAlloc {
    fn alloc(&mut self, size: u64) -> Result<u64, BtrfsError> {
        assert_eq!(size, self.nodesize, "a metadata block is always one nodesize");
        for run in self.runs.iter_mut() {
            if run.1 >= size {
                let at = run.0;
                run.0 += size;
                run.1 -= size;
                self.handed_out.push(at);
                return Ok(at);
            }
        }
        Err(BtrfsError::InvalidArg)
    }
    fn free(&mut self, logical: u64, _size: u64) {
        self.freed.push(logical);
    }
}

// Reading a tree under construction

type Reader<'a> = &'a mut dyn FnMut(u64) -> Result<Vec<u8>, BtrfsError>;

/// Every item in a tree through the transaction: what the volume will hold
/// after commit
fn items_of<A: BlockAlloc>(t: &Trans<A>, root: u64, read: Reader<'_>) -> Vec<(Key, Vec<u8>)> {
    let start = t.new_root_of(root).unwrap_or(root);
    let mut out = Vec::new();
    collect(t, start, read, &mut out, 0);
    out
}

fn collect<A: BlockAlloc>(
    t: &Trans<A>,
    logical: u64,
    read: Reader<'_>,
    out: &mut Vec<(Key, Vec<u8>)>,
    depth: usize,
) {
    assert!(depth <= MAX_LEVEL as usize, "a tree that deep is a loop");
    let node = t
        .node_at(logical, read)
        .unwrap_or_else(|e| panic!("block {logical} would be refused: {e:?}"));
    if node.is_leaf() {
        for slot in 0..node.nritems {
            out.push((node.key_at(slot).unwrap(), node.item_data(slot).unwrap().to_vec()));
        }
        return;
    }
    for slot in 0..node.nritems {
        let child = node.blockptr_at(slot).unwrap();
        collect(t, child, read, out, depth + 1);
    }
}

/// One item out of a tree that is still being built
fn item_of<A: BlockAlloc>(t: &Trans<A>, root: u64, key: &Key, read: Reader<'_>) -> Option<Vec<u8>> {
    items_of(t, root, read).into_iter().find(|(k, _)| k == key).map(|(_, d)| d)
}

/// Every metadata extent item in the tree, decoded, keyed by block address
fn extents_of<A: BlockAlloc>(
    t: &Trans<A>,
    ext: &ExtentTree,
    read: Reader<'_>,
) -> BTreeMap<u64, TreeBlockExtent> {
    let mut out = BTreeMap::new();
    for (k, d) in items_of(t, ext.root(), read) {
        if k.kind != METADATA_ITEM_KEY {
            continue;
        }
        let e = parse_tree_block(&k, &d)
            .unwrap_or_else(|err| panic!("an item this crate wrote does not parse: {err:?}"));
        assert!(out.insert(e.bytenr, e).is_none(), "two items for block {}", e.bytenr);
    }
    out
}

// The fixture: a real volume, a transaction, one real change

/// Inode of the fs tree's root directory. A fixed size, so changing a field is
/// the smallest change: nothing but the copied path
const ROOT_DIR_INO: u64 = 256;
const INODE_ITEM_KEY: u8 = 1;

struct Fixture {
    img: Image,
    vol: Volume<FileHost>,
    trans: Trans<ChunkAlloc>,
    ext: ExtentTree,
    /// The root tree's address on the medium
    root_tree: u64,
    /// The fs tree's address on the medium and its depth, as a one-level tree
    /// exercises nothing above a leaf
    #[allow(dead_code)]
    fs_tree: u64,
    fs_level: u8,
    nodesize: u32,
    generation: u64,
}

/// A volume and a transaction with one change made. `populate` goes to
/// `mkfs -r`, needing no root or mount, and gives an fs tree with more than one
/// level
fn fixture(tag: &str, populate: Option<&Path>) -> Fixture {
    let img = Image::new(tag, populate).expect(
        "mkfs.btrfs is missing. It is vendored at refsrc/btrfs-progs and built there; \
         a checkout without it cannot run this file, and a case that quietly passed \
         without its fixture is the defect this message exists to prevent",
    );

    let mut vol = Volume::open(FileHost::open(&img.path), 0).expect("a fresh volume must open");
    vol.load_chunk_tree().expect("the chunk tree must load");

    let g = vol.geometry;
    assert_ne!(
        g.incompat & FEATURE_INCOMPAT_SKINNY_METADATA,
        0,
        "mkfs.btrfs here sets SKINNY_METADATA and this whole file is about that form"
    );

    let space = read_free_space(&mut vol).expect("the free space tree must read");
    let alloc = ChunkAlloc::over(&space, g.nodesize as u64);

    let fsid = *<&[u8; FSID_SIZE]>::try_from(vol.superblock.fsid()).unwrap();
    let ext_root = vol.find_root(EXTENT_TREE_OBJECTID).expect("the extent tree must be there").bytenr;
    let ext = ExtentTree::new(ext_root, g.incompat).expect("a volume mkfs made must be supported");
    let fs = vol.fs_root().expect("the fs tree must be findable");
    let (fs_tree, fs_level) = (fs.bytenr, fs.level);

    let mut trans = Trans::new(alloc, g.nodesize, g.sectorsize, fsid, g.generation + 1);

    // The change: one field of one inode item, the same size, exercising only
    // the copied path, which produces blocks needing extent items
    let key = Key { objectid: ROOT_DIR_INO, kind: INODE_ITEM_KEY, offset: 0 };
    let mut body = vol.find_item(fs_tree, &key).expect("the root directory has an inode item");
    let at = btrfs::fs_tree::inode::MTIME;
    let was = u64_at(&body, at).unwrap();
    body[at..at + 8].copy_from_slice(&(was + 1).to_le_bytes());
    {
        let nodesize = g.nodesize as usize;
        let v = &mut vol;
        let mut read = |logical: u64| {
            let mut b = vec![0u8; nodesize];
            v.read_logical(logical, &mut b)?;
            Ok(b)
        };
        trans
            .update_item(fs_tree, &key, &body, &mut read)
            .expect("an in-place update of an existing item must work");
    }

    Fixture {
        img,
        vol,
        trans,
        ext,
        root_tree: g.root,
        fs_tree,
        fs_level,
        nodesize: g.nodesize,
        generation: g.generation + 1,
    }
}

impl Fixture {
    /// Runs `f` with a reader going to the volume for what the transaction has
    /// not touched
    fn with_read<R>(&mut self, f: impl FnOnce(&mut Trans<ChunkAlloc>, Reader<'_>) -> R) -> R {
        let nodesize = self.nodesize as usize;
        let vol = &mut self.vol;
        let mut read = |logical: u64| {
            let mut b = vec![0u8; nodesize];
            vol.read_logical(logical, &mut b)?;
            Ok(b)
        };
        f(&mut self.trans, &mut read)
    }
}

// The guard

/// Without btrfs-progs, cases judging bytes against btrfs's reading have no
/// judge
#[test]
fn the_external_judges_are_here() {
    common::require_tools();
}

// The bytes against `mkfs.btrfs`

/// Each metadata extent item on a `mkfs.btrfs` volume, re-encoded from its
/// block and required identical. Covers the key's offset, refcount, generation,
/// flags, backref type, and root, in the 33 bytes or 17-byte key.
///
/// The skinny form's key has the tree level where the old form has the extent
/// length; `__btrfs_free_extent` could not find a wrong one
#[test]
fn every_item_mkfs_wrote_is_the_item_this_crate_would_write() {
    let img = Image::new("mkfs-oracle", None).expect("mkfs.btrfs must be there");
    let mut vol = Volume::open(FileHost::open(&img.path), 0).expect("the volume must open");
    vol.load_chunk_tree().expect("the chunk tree must load");
    let nodesize = vol.geometry.nodesize as usize;

    let ext_root = vol.find_root(EXTENT_TREE_OBJECTID).expect("the extent tree is there").bytenr;
    let mut theirs: Vec<(Key, Vec<u8>)> = Vec::new();
    vol.for_each_item(ext_root, &mut |k, d| {
        if k.kind == METADATA_ITEM_KEY {
            theirs.push((k, d.to_vec()));
        }
        Ok(true)
    })
    .expect("the extent tree must walk");

    assert!(
        theirs.len() >= 8,
        "a fresh volume has an item per tree block and there are at least eight trees; \
         got {} - this case would otherwise be checking nothing",
        theirs.len()
    );

    for (key, item) in &theirs {
        // The block the item describes. Its header holds level, owning root,
        // and generation
        let mut raw = vec![0u8; nodesize];
        vol.read_logical(key.objectid, &mut raw)
            .unwrap_or_else(|e| panic!("block {} has an item and cannot be read: {e:?}", key.objectid));
        let level = u8_at(&raw, header::LEVEL).unwrap();
        let owner = u64_at(&raw, header::OWNER).unwrap();
        let generation = u64_at(&raw, header::GENERATION).unwrap();

        assert_eq!(
            skinny_key(key.objectid, level),
            *key,
            "the key for block {} does not match the one mkfs wrote. The offset of a \
             skinny metadata key is the tree level ({level}), not the extent length \
             ({nodesize})",
            key.objectid
        );
        assert_eq!(
            &encode_tree_block(generation, owner)[..],
            &item[..],
            "the item for block {} (level {level}, owner {owner}, generation {generation}) \
             is not the 33 bytes mkfs wrote",
            key.objectid
        );

        // And the decode, against the same three facts
        let got = parse_tree_block(key, item).expect("a real item must parse");
        assert_eq!(got.refs, 1, "a tree block mkfs allocated has exactly one reference");
        assert_eq!(got.level, level);
        assert_eq!(got.generation, generation);
        assert_eq!(got.flags, EXTENT_FLAG_TREE_BLOCK);
        assert_eq!(
            got.backref,
            Backref::Root(owner),
            "with MIXED_BACKREF an ordinary block's inline ref is a TREE_BLOCK_REF naming \
             the owning root, not a SHARED_BLOCK_REF naming a parent block"
        );
    }
}

/// A volume deep enough that the key's level is non-zero. On a fresh volume
/// every block is level zero, which a hard-coded zero would pass
#[test]
fn a_level_in_a_key_is_a_level_and_not_always_zero() {
    let mut stage = std::env::temp_dir();
    stage.push(format!("btrfs-extent-deep-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&stage);
    std::fs::create_dir_all(&stage).expect("cannot make the staging directory");
    for i in 0..4000 {
        std::fs::write(stage.join(format!("f{i:05}")), b"x").expect("cannot stage a file");
    }

    let img = Image::new("deep-levels", Some(&stage)).expect("mkfs.btrfs must be there");
    let _ = std::fs::remove_dir_all(&stage);

    let mut vol = Volume::open(FileHost::open(&img.path), 0).expect("the volume must open");
    vol.load_chunk_tree().expect("the chunk tree must load");
    let nodesize = vol.geometry.nodesize as usize;

    let ext_root = vol.find_root(EXTENT_TREE_OBJECTID).expect("the extent tree is there").bytenr;
    let mut theirs: Vec<(Key, Vec<u8>)> = Vec::new();
    vol.for_each_item(ext_root, &mut |k, d| {
        if k.kind == METADATA_ITEM_KEY {
            theirs.push((k, d.to_vec()));
        }
        Ok(true)
    })
    .expect("the extent tree must walk");

    let above_leaf = theirs.iter().filter(|(k, _)| k.offset > 0).count();
    assert!(
        above_leaf > 0,
        "this volume was built to have blocks above the leaves and has none, so the \
         level field is never anything but zero here - raise the file count"
    );

    for (key, item) in &theirs {
        let mut raw = vec![0u8; nodesize];
        vol.read_logical(key.objectid, &mut raw).expect("a described block must read");
        let level = u8_at(&raw, header::LEVEL).unwrap();
        let owner = u64_at(&raw, header::OWNER).unwrap();
        let generation = u64_at(&raw, header::GENERATION).unwrap();
        assert_eq!(skinny_key(key.objectid, level), *key, "block {}", key.objectid);
        assert_eq!(&encode_tree_block(generation, owner)[..], &item[..], "block {}", key.objectid);
    }
}

/// Forms not written are refused by name
#[test]
fn a_form_this_crate_does_not_write_is_refused_by_name() {
    let skinny = FEATURE_INCOMPAT_SKINNY_METADATA;
    let mixed = FEATURE_INCOMPAT_MIXED_BACKREF;

    assert!(ExtentTree::new(1 << 20, skinny | mixed).is_ok());
    assert_eq!(
        ExtentTree::new(1 << 20, mixed).unwrap_err(),
        BtrfsError::Unsupported,
        "without SKINNY_METADATA the item needs an EXTENT_ITEM key and a tree_block_info, \
         which this crate does not write"
    );
    assert_eq!(
        ExtentTree::new(1 << 20, skinny).unwrap_err(),
        BtrfsError::Unsupported,
        "without MIXED_BACKREF every backref would have to be a full backref naming a parent"
    );
    assert_eq!(
        ExtentTree::new(1 << 20, skinny | mixed | FEATURE_INCOMPAT_EXTENT_TREE_V2).unwrap_err(),
        BtrfsError::Unsupported
    );

    // The old key form, and a data extent, are the same refusal
    let old = Key { objectid: 1 << 20, kind: EXTENT_ITEM_KEY, offset: 16384 };
    let body = encode_tree_block(9, 5);
    assert_eq!(parse_tree_block(&old, &body).unwrap_err(), BtrfsError::Unsupported);

    // A level that is not a level
    let silly = Key { objectid: 1 << 20, kind: METADATA_ITEM_KEY, offset: 16384 };
    assert_eq!(parse_tree_block(&silly, &body).unwrap_err(), BtrfsError::Corrupt);
}

// Adding and removing in a transaction

/// An added block is described, matching its header
#[test]
fn adding_a_tree_block_writes_an_item_that_names_its_root() {
    let mut f = fixture("add", None);
    let ext = f.ext;
    let generation = f.generation;
    let at = 1u64 << 34;

    f.with_read(|t, read| {
        t.add_tree_block(&ext, at, 2, 7, read).expect("a fresh key must insert");
        let got = item_of(t, ext.root(), &skinny_key(at, 2), read)
            .expect("the item must be where the key says");
        let e = parse_tree_block(&skinny_key(at, 2), &got).expect("and must parse");
        assert_eq!(e.refs, 1);
        assert_eq!(e.generation, generation, "the item carries this transaction's generation");
        assert_eq!(e.flags, EXTENT_FLAG_TREE_BLOCK);
        assert_eq!(e.level, 2);
        assert_eq!(e.backref, Backref::Root(7), "the backref names the root that was asked for");
    });
}

/// Describing a block twice is refused
#[test]
fn a_block_cannot_be_described_twice() {
    let mut f = fixture("twice", None);
    let ext = f.ext;
    let at = 1u64 << 34;
    f.with_read(|t, read| {
        t.add_tree_block(&ext, at, 0, 5, read).expect("the first must work");
        assert_eq!(
            t.add_tree_block(&ext, at, 0, 5, read).unwrap_err(),
            BtrfsError::InvalidArg,
            "a second item for one block is a refcount of two written as two items"
        );
    });
}

/// An address no reader could use is refused before writing
#[test]
fn an_address_or_a_level_the_format_cannot_hold_is_refused() {
    let mut f = fixture("bounds", None);
    let ext = f.ext;
    f.with_read(|t, read| {
        assert_eq!(
            t.add_tree_block(&ext, (1u64 << 34) + 1, 0, 5, read).unwrap_err(),
            BtrfsError::InvalidArg,
            "check_extent_item wants the bytenr sector aligned"
        );
        assert_eq!(
            t.add_tree_block(&ext, 1u64 << 34, MAX_LEVEL, 5, read).unwrap_err(),
            BtrfsError::InvalidArg,
            "check_extent_item wants the level below BTRFS_MAX_LEVEL"
        );
    });
}

/// Removing: the item is gone and the address undescribed
#[test]
fn dropping_a_tree_block_takes_its_item_out() {
    let mut f = fixture("drop", None);
    let ext = f.ext;
    let at = 1u64 << 34;
    f.with_read(|t, read| {
        t.add_tree_block(&ext, at, 1, 5, read).expect("add");
        assert!(item_of(t, ext.root(), &skinny_key(at, 1), read).is_some());
        t.drop_tree_block(&ext, at, 1, read).expect("drop");
        assert!(
            item_of(t, ext.root(), &skinny_key(at, 1), read).is_none(),
            "the item is still there after the block was freed, which is a block the next \
             mount thinks is in use forever"
        );
        assert_eq!(
            t.drop_tree_block(&ext, at, 1, read).unwrap_err(),
            BtrfsError::NotFound,
            "dropping a reference that is not there is an error, not a silence"
        );
    });
}

// The fixpoint

/// After the loop, the extent tree describes exactly the blocks the new tree
/// has. A settle missing the blocks its own items produced would leave the
/// extent tree's new blocks undescribed, and allocated over at next mount
#[test]
fn the_loop_settles_and_describes_every_block_it_leaves_behind() {
    let mut f = fixture("settle", None);
    let ext = f.ext;
    let generation = f.generation;

    let passes = f
        .with_read(|t, read| t.settle_extent_tree(&ext, read))
        .expect("a transaction this small must settle");
    assert!(passes >= 2, "a settle that took {passes} passes did not have to chase itself");

    f.with_read(|t, read| {
        let described = extents_of(t, &ext, read);
        let held: Vec<u64> = t.dirty_blocks().map(|(b, _)| b).collect();
        assert!(!held.is_empty(), "nothing was built, so nothing was checked");

        for b in &held {
            let e = described
                .get(b)
                .unwrap_or_else(|| panic!("block {b} exists and has no extent item - the next \
                                           mount would call that address free and write over it"));
            let raw = t.block_bytes(*b, read).expect("a held block reads back");
            assert_eq!(e.level, u8_at(&raw, header::LEVEL).unwrap(), "block {b}: level");
            assert_eq!(
                e.backref,
                Backref::Root(u64_at(&raw, header::OWNER).unwrap()),
                "block {b}: the backref must name the root in the block's own header"
            );
            assert_eq!(e.refs, 1, "block {b}: refcount");
            assert_eq!(e.generation, generation, "block {b}: generation");
        }

        for b in t.freed_blocks() {
            assert!(
                !described.contains_key(&b),
                "block {b} was left behind by copy-on-write and still has an extent item, \
                 so the volume claims a block the new tree does not reference"
            );
        }

        // More than one tree was touched, so the backref check is meaningful
        let owners: std::collections::BTreeSet<u64> = held
            .iter()
            .map(|b| u64_at(&t.block_bytes(*b, read).unwrap(), header::OWNER).unwrap())
            .collect();
        assert!(
            owners.len() >= 2,
            "every new block belongs to one tree ({owners:?}), so a backref hard-coded to \
             that root would pass"
        );
    });
}

/// A loop that will not settle is an error, through the bound
#[test]
fn a_settle_that_runs_out_of_passes_is_an_error() {
    let mut f = fixture("ceiling", None);
    let ext = f.ext;
    let e = f
        .with_read(|t, read| t.settle_extent_tree_within(&ext, 1, read))
        .expect_err("one pass cannot settle a transaction that copies the extent tree");
    assert_eq!(e, BtrfsError::Corrupt);
}

/// Passes share state, so the free space half can reach the same fixpoint
#[test]
fn a_pass_reports_what_it_changed_and_stops_changing_things() {
    let mut f = fixture("passes", None);
    let ext = f.ext;
    f.with_read(|t, read| {
        let mut state = ExtentSettle::new();
        let first = t.extent_pass(&ext, &mut state, read).expect("a first pass");
        assert!(first > 0, "the transaction had a change in it and the first pass found none");
        let mut last = first;
        let mut rounds = 1;
        while last > 0 {
            last = t.extent_pass(&ext, &mut state, read).expect("a later pass");
            rounds += 1;
            assert!(rounds < 16, "this did not converge");
        }
        assert_eq!(
            state.described() - state.removed(),
            0,
            "a pure copy-on-write transaction replaces each block it leaves behind, so the \
             two counts match; they were {} and {}",
            state.described(),
            state.removed()
        );
    });
}

// The judge

/// Everything a commit moves once the trees have. The transaction-level
/// fixpoint, belonging to the commit tier: settling the extent tree moves its
/// root item, which moves the root tree, which needs settling.
/// `settle_extent_tree` is the inner loop, this the outer; hence the public
/// `ExtentSettle`
fn commit_one_change(tag: &str, populate: Option<&Path>) -> (Image, String, usize, usize, u8) {
    let mut f = fixture(tag, populate);
    let root_tree = f.root_tree;
    let ext = f.ext;
    let generation = f.generation;
    let nodesize = f.nodesize;

    // Every root the root tree names, by its medium address; moved ones must
    // report their place
    let mut roots: Vec<(Key, u64)> = Vec::new();
    f.vol
        .for_each_item(root_tree, &mut |k, d| {
            if k.kind == ROOT_ITEM_KEY {
                roots.push((k, Root::parse(d)?.bytenr));
            }
            Ok(true)
        })
        .expect("the root tree must walk");
    assert!(roots.len() >= 5, "a volume has more roots than this: {}", roots.len());

    let (sealed, described, removed) = f.with_read(|t, read| {
        let mut state = ExtentSettle::new();
        let mut placed: BTreeMap<u64, u64> = BTreeMap::new();
        let mut settled = false;
        for _ in 0..MAX_SETTLE_PASSES {
            let mut changed = t.extent_pass(&ext, &mut state, read).expect("an extent pass");

            for (key, was) in &roots {
                let Some(now) = t.new_root_of(*was) else { continue };
                if placed.get(was) == Some(&now) {
                    continue;
                }
                let raw = t.block_bytes(now, read).expect("a moved root must be readable");
                let level = u8_at(&raw, header::LEVEL).unwrap();
                let mut body = item_of(t, root_tree, key, read)
                    .unwrap_or_else(|| panic!("the root item for {key:?} vanished"));
                body[root_item::BYTENR..root_item::BYTENR + 8]
                    .copy_from_slice(&now.to_le_bytes());
                body[root_item::GENERATION..root_item::GENERATION + 8]
                    .copy_from_slice(&generation.to_le_bytes());
                body[root_item::LEVEL] = level;
                t.update_item(root_tree, key, &body, read).expect("a root item is a fixed size");
                placed.insert(*was, now);
                changed += 1;
            }

            if changed == 0 {
                settled = true;
                break;
            }
        }
        assert!(settled, "the commit did not settle inside {MAX_SETTLE_PASSES} passes");
        (
            t.seal().expect("every block must seal"),
            state.described(),
            state.removed(),
        )
    });

    let new_root_tree = f
        .trans
        .new_root_of(root_tree)
        .expect("the root tree moved, because its items did");

    for (logical, data) in &sealed {
        write_block(&mut f.vol, *logical, data).expect("a sealed block must write");
    }

    // The superblock names the root tree, which no root item holds.
    // `bytes_used` moves by blocks added and given up
    let level = f.vol.read_node(new_root_tree).expect("the new root tree must read").level;
    let used = u64_at(&f.vol.superblock.data, sb::BYTES_USED).unwrap();
    let used = used + nodesize as u64 * described as u64 - nodesize as u64 * removed as u64;
    f.vol.superblock.data[sb::ROOT..sb::ROOT + 8].copy_from_slice(&new_root_tree.to_le_bytes());
    f.vol.superblock.data[sb::ROOT_LEVEL] = level;
    f.vol.superblock.data[sb::BYTES_USED..sb::BYTES_USED + 8].copy_from_slice(&used.to_le_bytes());
    // The copy must pass `check_superblock` before `commit_superblocks`, and
    // three fields just changed under its checksum
    let csum = f.vol.superblock.computed_csum();
    f.vol.superblock.data[sb::CSUM..sb::CSUM + 4].copy_from_slice(&csum.to_le_bytes());

    commit_superblocks(&mut f.vol, generation).expect("the superblocks must commit");

    // Rule 1 at real addresses. `Trans::check_fresh` refuses what it is
    // offered; this is the allocator's own record
    let alloc = &f.trans.alloc;
    let reused: Vec<u64> = alloc
        .handed_out
        .iter()
        .copied()
        .filter(|a| alloc.freed.contains(a))
        .collect();
    assert!(reused.is_empty(), "the allocator handed back pinned blocks: {reused:?}");
    let mut seen = alloc.handed_out.clone();
    seen.sort_unstable();
    let before = seen.len();
    seen.dedup();
    assert_eq!(before, seen.len(), "the allocator handed out one address twice");

    drop(f.vol);

    let out = f.img.btrfs(&["check"]);
    (f.img, out, described, removed, f.fs_level)
}

/// Lines `btrfs check` prints under each of eight phases, per phase, so it says
/// which pass is clean
fn complaints_by_phase(out: &str) -> Vec<(String, Vec<String>)> {
    let mut phases: Vec<(String, Vec<String>)> = Vec::new();
    for line in out.lines() {
        if line.starts_with('[') && line.contains("/8]") {
            phases.push((line.to_string(), Vec::new()));
        } else if let Some(last) = phases.last_mut() {
            if !line.trim().is_empty() {
                last.1.push(line.to_string());
            }
        }
    }
    assert_eq!(
        phases.len(),
        8,
        "btrfs check did not run its eight phases, so this case judged nothing:\n{out}"
    );
    phases
}

/// The tier judged by `btrfs check` on a volume committed here.
/// `[3/8] checking extents` walks each item against real blocks and backrefs.
///
/// The rest shows under `[4/8] checking free space tree`: the free space tree
/// and block group accounting, which the kernel does in
/// `do_free_extent_accounting` with `add_to_free_space_tree` and
/// `btrfs_update_block_group`. So every phase but that one must be clean
fn judge(tag: &str, populate: Option<&Path>, want_fs_level: u8) {
    let (_img, out, described, removed, fs_level) = commit_one_change(tag, populate);
    assert_eq!(
        fs_level, want_fs_level,
        "this case was built for an fs tree {want_fs_level} levels deep and got one \
         {fs_level} deep, so it is not exercising what it says it is"
    );
    eprintln!("--- btrfs check, {tag} ---\n{out}--- end ---");
    assert_eq!(
        described, removed,
        "a pure copy-on-write transaction gives up one block for each it takes"
    );
    assert!(
        described >= 3,
        "only {described} blocks were described, which is fewer than a commit has to move \
         (the tree that changed, the extent tree and the root tree)"
    );

    for (phase, lines) in complaints_by_phase(&out) {
        if phase.contains("free space tree") {
            // Named: the tier's other half
            continue;
        }
        assert!(
            lines.is_empty(),
            "btrfs check disagrees under {phase}:\n{}\n\nfull output:\n{out}",
            lines.join("\n")
        );
    }
}

/// A fresh volume: the fs tree is one leaf, so the change copies one block and
/// the commit moves three
#[test]
fn btrfs_check_judges_a_volume_this_crate_committed() {
    judge("judged", None, 0);
}

/// A volume deep enough that the change copies an internal node too
#[test]
fn btrfs_check_judges_a_volume_whose_trees_have_parents() {
    let mut stage = std::env::temp_dir();
    stage.push(format!("btrfs-extent-judged-deep-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&stage);
    std::fs::create_dir_all(&stage).expect("cannot make the staging directory");
    for i in 0..4000 {
        std::fs::write(stage.join(format!("f{i:05}")), b"x").expect("cannot stage a file");
    }
    judge("judged-deep", Some(&stage), 1);
    let _ = std::fs::remove_dir_all(&stage);
}

// Both halves together

/// The same commit with the free space tree and block groups written:
/// `Trans::extent_pass` from `src/extent.rs`, `space::settle` from
/// `src/space.rs`, and root items, in one outer loop until a round changes
/// nothing.
///
/// The outer loop is here: settling one half moves blocks the other must
/// describe, so neither can be outer. It is the commit tier's
fn commit_both_halves(tag: &str, populate: Option<&Path>) -> (Image, String) {
    let img = Image::new(tag, populate).expect("mkfs.btrfs must be there");
    let mut vol = Volume::open(FileHost::open(&img.path), 0).expect("the volume must open");
    vol.load_chunk_tree().expect("the chunk tree must load");

    let g = vol.geometry;
    let generation = g.generation + 1;
    let root_tree = g.root;
    let fsid = *<&[u8; FSID_SIZE]>::try_from(vol.superblock.fsid()).unwrap();
    let ext = ExtentTree::new(
        vol.find_root(EXTENT_TREE_OBJECTID).expect("an extent tree").bytenr,
        g.incompat,
    )
    .expect("a volume mkfs made must be supported");
    let fst_root = vol.find_root(FREE_SPACE_TREE_OBJECTID).expect("a free space tree").bytenr;
    let bg_root = if g.compat_ro & FEATURE_COMPAT_RO_BLOCK_GROUP_TREE != 0 {
        vol.find_root(BLOCK_GROUP_TREE_OBJECTID).expect("a block group tree").bytenr
    } else {
        ext.root()
    };

    let alloc = allocator_for(&mut vol).expect("the allocator must build over this volume");
    let mut trans = Trans::new(alloc, g.nodesize, g.sectorsize, fsid, generation);

    let key = Key { objectid: ROOT_DIR_INO, kind: INODE_ITEM_KEY, offset: 0 };
    let fs_tree = vol.fs_root().expect("an fs tree").bytenr;
    let mut body = vol.find_item(fs_tree, &key).expect("the root directory has an inode item");
    let at = btrfs::fs_tree::inode::MTIME;
    let was = u64_at(&body, at).unwrap();
    body[at..at + 8].copy_from_slice(&(was + 1).to_le_bytes());

    let (sealed, used, rounds) = {
        let nodesize = g.nodesize as usize;
        let v = &mut vol;
        let mut read = |logical: u64| {
            let mut b = vec![0u8; nodesize];
            v.read_logical(logical, &mut b)?;
            Ok(b)
        };
        let read: Reader<'_> = &mut read;

        trans.update_item(fs_tree, &key, &body, read).expect("the change must apply");

        let mut state = ExtentSettle::new();
        let mut rounds = 0usize;
        let mut settled = false;
        for _ in 0..MAX_SETTLE_PASSES {
            rounds += 1;
            let mut changed = trans.extent_pass(&ext, &mut state, read).expect("an extent pass");

            // Free space tree, block groups, and root items, with its own
            // fixpoint; any work shows as newly held blocks
            let before = trans.dirty_count();
            space_settle(&mut trans, fst_root, bg_root, root_tree, read)
                .expect("the free space half must settle");
            changed += trans.dirty_count() - before;

            if changed == 0 {
                settled = true;
                break;
            }
        }
        assert!(settled, "the two halves together did not settle in {MAX_SETTLE_PASSES} rounds");
        let used = trans.alloc.bytes_used();
        (trans.seal().expect("every block must seal"), used, rounds)
    };
    assert!(rounds >= 2, "a single round means the two halves never fed each other");

    let new_root_tree = trans.new_root_of(root_tree).expect("the root tree moved");
    for (logical, data) in &sealed {
        write_block(&mut vol, *logical, data).expect("a sealed block must write");
    }

    let level = vol.read_node(new_root_tree).expect("the new root tree must read").level;
    vol.superblock.data[sb::ROOT..sb::ROOT + 8].copy_from_slice(&new_root_tree.to_le_bytes());
    vol.superblock.data[sb::ROOT_LEVEL] = level;
    vol.superblock.data[sb::BYTES_USED..sb::BYTES_USED + 8].copy_from_slice(&used.to_le_bytes());
    let csum = vol.superblock.computed_csum();
    vol.superblock.data[sb::CSUM..sb::CSUM + 4].copy_from_slice(&csum.to_le_bytes());

    commit_superblocks(&mut vol, generation).expect("the superblocks must commit");
    drop(vol);

    let out = img.btrfs(&["check"]);
    (img, out)
}

/// This tier's gate: `btrfs check` reports the volume clean
#[test]
fn btrfs_check_reports_a_volume_both_halves_committed_clean() {
    judge_clean("both", None);
}

/// The same on a volume with trees above their leaves: copying internal nodes,
/// repointing parents, describing blocks above level zero
#[test]
fn btrfs_check_reports_a_deeper_volume_both_halves_committed_clean() {
    let mut stage = std::env::temp_dir();
    stage.push(format!("btrfs-extent-both-deep-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&stage);
    std::fs::create_dir_all(&stage).expect("cannot make the staging directory");
    for i in 0..4000 {
        std::fs::write(stage.join(format!("f{i:05}")), b"x").expect("cannot stage a file");
    }
    judge_clean("both-deep", Some(&stage));
    let _ = std::fs::remove_dir_all(&stage);
}

fn judge_clean(tag: &str, populate: Option<&Path>) {
    let (_img, out) = commit_both_halves(tag, populate);
    eprintln!("--- btrfs check, both halves, {tag} ---\n{out}--- end ---");
    for (phase, lines) in complaints_by_phase(&out) {
        assert!(lines.is_empty(), "btrfs check disagrees under {phase}:\n{}", lines.join("\n"));
    }
    // btrfs-progs's words for a clean volume. The per-phase check says where a
    // complaint was; this is the summary line
    assert!(
        out.contains("no error found"),
        "btrfs check did not call this volume clean:\n{out}"
    );
}
