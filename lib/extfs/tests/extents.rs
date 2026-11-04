// Extent trees fed bad images on purpose. An extent header carries its entry
// count from disk, bounding a loop over 60 bytes in an inode or one block in a
// node, so every one of these must be refused

use extfs::mkfs::{mkfs, FsType, MkfsParams};
use extfs::structs::*;
use extfs::{MemDisk, MikuFS};

const SECTORS: usize = 128 * 1024;

fn ext4_fs() -> MemDisk {
    let mut disk = MemDisk::new(SECTORS);
    let mut params = MkfsParams::new(FsType::Ext4, 0);
    params.total_sectors = SECTORS as u32;
    mkfs(&mut disk, &params).expect("mkfs failed");
    disk
}

/// An inode with an extent header whose fields we control
fn inode_with_header(entries: u16, max: u16, depth: u16) -> Inode {
    let mut inode = Inode::zeroed();
    inode.on_disk_size = 256;
    inode.set_mode(S_IFREG | 0o644);
    inode.set_flags(EXT4_EXTENTS_FL);
    // The header lives at the start of i_block, offset 40
    inode.write_u16(40, EXT4_EXT_MAGIC);
    inode.write_u16(42, entries);
    inode.write_u16(44, max);
    inode.write_u16(46, depth);
    inode.write_u32(48, 0);
    inode
}

// The header check

/// Only four extents fit in an inode's 60 bytes: the 12-byte header, then 12
/// bytes each
#[test]
fn an_inode_root_holds_four_extents() {
    assert_eq!(ExtentHeader::capacity(EXTENT_ROOT_SPACE), 4);
}

#[test]
fn a_well_formed_root_passes() {
    let h = ExtentHeader { magic: EXT4_EXT_MAGIC, entries: 3, max: 4, depth: 0, generation: 0 };
    assert!(h.check(EXTENT_ROOT_SPACE, None));
}

#[test]
fn the_wrong_magic_is_refused() {
    let h = ExtentHeader { magic: 0x1234, entries: 1, max: 4, depth: 0, generation: 0 };
    assert!(!h.check(EXTENT_ROOT_SPACE, None));
}

/// 18 entries puts the last at byte 268 of a 256-byte inode
#[test]
fn more_entries_than_the_inode_holds_is_refused() {
    for entries in [5u16, 18, 100, u16::MAX] {
        let h = ExtentHeader {
            magic: EXT4_EXT_MAGIC,
            entries,
            max: entries,
            depth: 0,
            generation: 0,
        };
        assert!(
            !h.check(EXTENT_ROOT_SPACE, None),
            "an inode root claiming {entries} entries must be refused"
        );
    }
}

/// `entries` must not exceed `max`
#[test]
fn entries_beyond_max_is_refused() {
    let h = ExtentHeader { magic: EXT4_EXT_MAGIC, entries: 4, max: 2, depth: 0, generation: 0 };
    assert!(!h.check(EXTENT_ROOT_SPACE, None));
}

/// ext4's limit is five; deeper means recursion with a block-sized buffer per
/// level
#[test]
fn an_absurd_depth_is_refused() {
    for depth in [6u16, 100, u16::MAX] {
        let h = ExtentHeader {
            magic: EXT4_EXT_MAGIC,
            entries: 1,
            max: 4,
            depth,
            generation: 0,
        };
        assert!(!h.check(EXTENT_ROOT_SPACE, None), "depth {depth} must be refused");
    }
}

/// A depth other than the parent's promise stops a walk from following a cycle
#[test]
fn a_node_at_the_wrong_depth_is_refused() {
    let h = ExtentHeader { magic: EXT4_EXT_MAGIC, entries: 1, max: 4, depth: 2, generation: 0 };
    assert!(h.check(EXTENT_ROOT_SPACE, Some(2)));
    assert!(!h.check(EXTENT_ROOT_SPACE, Some(1)));
    assert!(!h.check(EXTENT_ROOT_SPACE, Some(0)));
}

/// A 4 KiB node holds many more than an inode, so the check scales with the
/// space
#[test]
fn capacity_scales_with_the_space() {
    assert_eq!(ExtentHeader::capacity(1024), 84);
    assert_eq!(ExtentHeader::capacity(4096), 340);
    assert_eq!(ExtentHeader::capacity(0), 0);
    assert_eq!(ExtentHeader::capacity(11), 0);
}

// Through the real walkers

/// The walkers refuse a hostile inode
#[test]
fn a_hostile_root_does_not_reach_the_walker() {
    let mut disk = ext4_fs();
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();

    let inode = inode_with_header(200, 200, 0);
    assert!(
        fs.get_file_block_extent(&inode, 0).is_err(),
        "a root claiming 200 extents must be refused, not walked"
    );
}

#[test]
fn a_hostile_root_is_refused_by_every_walker() {
    let mut disk = ext4_fs();
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();

    let inode = inode_with_header(9999, 9999, 3);

    // Read path
    assert!(fs.get_file_block_extent(&inode, 0).is_err());
    // Free path: nothing to free
    assert_eq!(fs.ext4_free_extent_blocks(&inode).unwrap(), 0);
    // Count path: same
    assert_eq!(fs.ext4_extent_count(&inode).unwrap(), 0);
}

/// A child pointer outside the filesystem is not followed
#[test]
fn a_child_pointer_outside_the_filesystem_is_refused() {
    let mut disk = ext4_fs();
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();

    let mut buf = [0u8; 4096];
    let past_the_end = fs.superblock.blocks_count();
    assert!(fs.read_extent_node(past_the_end, None, None, &mut buf).is_err());
    assert!(fs.read_extent_node(past_the_end + 1000, None, None, &mut buf).is_err());
    assert!(fs.read_extent_node(0, None, None, &mut buf).is_err());
}

/// A block inside the filesystem holding something other than an extent node,
/// such as a data block or bitmap, is refused too
#[test]
fn a_block_that_is_not_a_node_is_refused() {
    let mut disk = ext4_fs();
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();

    let blk = fs.alloc_block(0).unwrap();
    let bs = fs.block_size as usize;
    fs.write_block_data(blk, &[0x5A; 4096][..bs]).unwrap();

    let mut buf = [0u8; 4096];
    assert!(fs.read_extent_node(blk, None, None, &mut buf).is_err());
}

/// A real file's extents are readable
#[test]
fn a_real_extent_file_still_reads() {
    let mut disk = ext4_fs();
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();

    let ino = fs.ext4_create_file(2, "data.bin", 0o644).unwrap();
    let payload: alloc::vec::Vec<u8> = (0..20_000u32).map(|i| i as u8).collect();
    fs.ext4_write_file(ino, &payload, 0).unwrap();

    let inode = fs.read_inode(ino).unwrap();
    assert!(inode.uses_extents(), "ext4 must write this file with extents");
    let header = fs.extent_root(&inode).expect("our own tree must validate");
    assert!(header.entries > 0);

    let mut back = alloc::vec![0u8; payload.len()];
    let n = fs.read_file(&inode, 0, &mut back).unwrap();
    assert_eq!(n, payload.len());
    assert_eq!(back, payload, "what was written must read back");
}

extern crate alloc;

// Growing a tree: depth three and beyond, deepening when a leaf and root are
// both full

/// Interleaved writes across files, so each block becomes its own extent,
/// filling a tree quickly
fn interleaved(fs: &mut MikuFS<&mut MemDisk>, files: usize, blocks_each: u64) -> Vec<u32> {
    let bs = fs.block_size as usize;
    let mut inos = Vec::new();
    for i in 0..files {
        inos.push(
            fs.ext4_create_file(2, &format!("deep-{i}.bin"), 0o644)
                .expect("create failed"),
        );
    }
    for round in 0..blocks_each {
        for (i, &ino) in inos.iter().enumerate() {
            // Content that identifies both the file and the block
            let mut block = vec![0u8; bs];
            block[0] = i as u8;
            block[1..5].copy_from_slice(&(round as u32).to_le_bytes());
            fs.ext4_write_file(ino, &block, round * bs as u64)
                .expect("write failed");
        }
    }
    inos
}

fn small_block_ext4() -> MemDisk {
    let mut disk = MemDisk::new(SECTORS);
    let mut params = MkfsParams::new(FsType::Ext4, 0);
    params.total_sectors = SECTORS as u32;
    // 1 KiB blocks put 84 extents in a node instead of 340, reaching three
    // levels at a few hundred blocks
    params.block_size = 1024;
    mkfs(&mut disk, &params).expect("mkfs failed");
    disk
}

#[test]
fn a_tree_grows_past_two_levels() {
    let mut disk = small_block_ext4();
    let mut fs = MikuFS::open(&mut disk, 0, false).expect("mount failed");

    // The root holds 4 index entries and a 1 KiB node 84 extents, so two levels
    // hold at most 4 * 84 = 336
    let inos = interleaved(&mut fs, 4, 400);

    let depths: Vec<u16> = inos
        .iter()
        .map(|&ino| {
            let inode = fs.read_inode(ino).unwrap();
            fs.extent_root(&inode).unwrap().depth
        })
        .collect();
    assert!(
        depths.iter().any(|&d| d >= 2),
        "no file needed a three-level tree: depths {depths:?}"
    );
}

#[test]
fn every_block_of_a_deep_tree_reads_back() {
    let mut disk = small_block_ext4();
    let mut fs = MikuFS::open(&mut disk, 0, false).expect("mount failed");
    let bs = fs.block_size as usize;
    let inos = interleaved(&mut fs, 4, 400);

    // Every logical block still maps to the block its contents were written to
    for (i, &ino) in inos.iter().enumerate() {
        let inode = fs.read_inode(ino).unwrap();
        for round in 0..400u64 {
            let mut back = vec![0u8; bs];
            let n = fs
                .read_file(&inode, round * bs as u64, &mut back)
                .unwrap_or_else(|e| panic!("read file {i} block {round}: {e:?}"));
            assert_eq!(n, bs, "short read of file {i} block {round}");
            assert_eq!(back[0], i as u8, "file {i} block {round} came from file {}", back[0]);
            assert_eq!(
                u32::from_le_bytes([back[1], back[2], back[3], back[4]]),
                round as u32,
                "file {i} block {round} came from another block"
            );
        }
    }
}

/// The same tree after a remount, checking the tree on disk
#[test]
fn a_deep_tree_survives_a_remount() {
    let mut disk = small_block_ext4();
    let inos;
    {
        let mut fs = MikuFS::open(&mut disk, 0, false).expect("mount failed");
        inos = interleaved(&mut fs, 4, 400);
        fs.sync().expect("sync failed");
    }
    let mut fs = MikuFS::open(&mut disk, 0, false).expect("remount failed");
    let bs = fs.block_size as usize;
    for (i, &ino) in inos.iter().enumerate() {
        let inode = fs.read_inode(ino).unwrap();
        assert_eq!(inode.size(), 400 * bs as u64);
        let mut back = vec![0u8; bs];
        for round in [0u64, 199, 399] {
            fs.read_file(&inode, round * bs as u64, &mut back).unwrap();
            assert_eq!(back[0], i as u8);
            assert_eq!(
                u32::from_le_bytes([back[1], back[2], back[3], back[4]]),
                round as u32
            );
        }
    }
}

/// The page cache's write-out: pages handed one by one, the first run on fresh
/// blocks filled in place of zeros, the second over them off a block boundary.
/// The parts read back end to end, also after a remount, and the volume checks
/// clean
#[test]
fn pages_written_as_parts_read_back_whole() {
    use extfs::fsck::FsckOptions;
    let mut disk = ext4_fs();
    let page = |seed: u32, len: usize| -> Vec<u8> { (0..len as u32).map(|i| (i * 7 + seed) as u8).collect() };
    let mut want: Vec<u8>;
    let ino;
    {
        let mut fs = MikuFS::open(&mut disk, 0, false).expect("mount failed");
        ino = fs.ext4_create_file(2, "parts.bin", 0o644).unwrap();

        let fresh = [page(1, 4096), page(2, 4096), page(3, 4096), page(4, 1000)];
        let parts: Vec<&[u8]> = fresh.iter().map(|p| p.as_slice()).collect();
        assert_eq!(fs.ext3_write_parts(ino, &parts, 0).unwrap(), 3 * 4096 + 1000);
        want = fresh.concat();

        let over = [page(5, 3000), page(6, 4096), page(7, 500)];
        let parts: Vec<&[u8]> = over.iter().map(|p| p.as_slice()).collect();
        assert_eq!(fs.ext3_write_parts(ino, &parts, 2048).unwrap(), 3000 + 4096 + 500);
        let joined = over.concat();
        want[2048..2048 + joined.len()].copy_from_slice(&joined);

        let report = fs.fsck(FsckOptions::check()).expect("the check could not run");
        assert!(report.is_clean(), "not consistent: {:?}", report.examples());
        fs.sync().expect("sync failed");
    }
    let mut fs = MikuFS::open(&mut disk, 0, false).expect("remount failed");
    let inode = fs.read_inode(ino).unwrap();
    assert_eq!(inode.size(), want.len() as u64);
    let mut back = vec![0u8; want.len()];
    assert_eq!(fs.read_file(&inode, 0, &mut back).unwrap(), want.len());
    assert!(back == want, "the parts did not read back as written");
}
