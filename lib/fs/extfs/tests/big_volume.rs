// Volumes with more than 32 block groups. At 1 KiB blocks, 32 groups is only
// 256 MiB, so the case fits in a test

use extfs::fsck::{FsckOptions, FsckReport};
use extfs::mkfs::{mkfs, FsType, MkfsParams};
use extfs::{MemDisk, MikuFS};

/// Everything the check reported, for the failure message
fn complaints(r: &FsckReport) -> String {
    let mut out = String::new();
    for f in r.examples() {
        out.push_str(&alloc::format!("  {}\n", f.problem));
    }
    out
}

/// 512 MiB at 1 KiB blocks: 64 groups
const SECTORS: usize = 1024 * 1024;

fn big_fs() -> MemDisk {
    let mut disk = MemDisk::new(SECTORS);
    let mut params = MkfsParams::new(FsType::Ext2, 0);
    params.total_sectors = SECTORS as u32;
    mkfs(&mut disk, &params).expect("mkfs failed");
    disk
}

#[test]
fn the_image_really_has_more_than_32_groups() {
    let mut disk = big_fs();
    let fs = MikuFS::open(&mut disk, 0, false).unwrap();
    assert!(
        fs.group_count > 32,
        "test is pointless below the old ceiling: got {} groups",
        fs.group_count
    );
    assert_eq!(fs.groups.len(), fs.group_count as usize);
}

/// A block in a high group can be allocated, freed, and reallocated
#[test]
fn blocks_past_group_32_round_trip() {
    let mut disk = big_fs();
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();

    let group = 40;
    assert!(group < fs.groups.len(), "need a group past the old ceiling");

    let blk = fs
        .alloc_block(group)
        .expect("allocating in a high group must work");

    // It must come from up there, not a low-group fallback
    let first = fs.superblock.first_data_block();
    let landed = ((blk - first) / fs.blocks_per_group) as usize;
    assert!(
        landed >= 32,
        "asked for group {group}, got a block in group {landed} - the \
         allocator is still skipping high groups"
    );

    fs.free_block(blk).expect("freeing a high block must work");
}

/// Freeing accounts for the block
#[test]
fn freeing_a_high_block_updates_the_counters() {
    let mut disk = big_fs();
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();

    let group = 45;
    let before_group = fs.groups[group].free_blocks();
    let before_sb = fs.superblock.free_blocks_count();

    let blk = fs.alloc_block(group).unwrap();
    assert_eq!(fs.groups[group].free_blocks(), before_group - 1);
    assert_eq!(fs.superblock.free_blocks_count(), before_sb - 1);

    fs.free_block(blk).unwrap();
    assert_eq!(fs.groups[group].free_blocks(), before_group);
    assert_eq!(fs.superblock.free_blocks_count(), before_sb);
}

/// An inode in a high group is allocated and accounted for, or not allocated.
/// `alloc_inode` writes the bitmap bit before the counters, so an error must
/// not leave the bit set
#[test]
fn inodes_past_group_32_do_not_leak() {
    let mut disk = big_fs();
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();

    let group = 50;
    assert!(group < fs.groups.len());
    let before = fs.groups[group].free_inodes();

    let ino = fs.alloc_inode(group).expect("allocating a high inode must work");

    let landed = ((ino - 1) / fs.inodes_per_group) as usize;
    assert!(landed >= 32, "expected a high group, landed in {landed}");
    assert_eq!(
        fs.groups[landed].free_inodes(),
        before - 1,
        "the bit was written but not accounted for - this is the leak"
    );

    fs.free_inode(ino).unwrap();
    assert_eq!(fs.groups[landed].free_inodes(), before);
}

/// Every group is reachable: takes one block from each
#[test]
fn every_group_can_be_allocated_from() {
    let mut disk = big_fs();
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();

    let groups = fs.groups.len();
    let mut taken = alloc::vec::Vec::new();
    for g in 0..groups {
        if fs.groups[g].free_blocks() == 0 {
            continue;
        }
        let blk = fs
            .alloc_block(g)
            .unwrap_or_else(|e| panic!("group {g} of {groups} refused: {e:?}"));
        taken.push(blk);
    }
    assert!(
        taken.len() > 32,
        "only {} groups yielded a block",
        taken.len()
    );
    for blk in taken {
        fs.free_block(blk).unwrap();
    }
}

/// fsck covers the whole filesystem, including group 33 and beyond
#[test]
fn fsck_examines_every_group() {
    let mut disk = big_fs();
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();

    let clean = fs.fsck(FsckOptions::check()).unwrap();
    assert_eq!(
        clean.errors(), 0,
        "a freshly made filesystem must be clean:\n{}", complaints(&clean)
    );

    // Break a group descriptor past the old ceiling
    let victim = 40;
    assert!(victim < fs.groups.len());
    fs.groups[victim].write_u32(0, 0); // block_bitmap = 0 is impossible

    let dirty = fs.fsck(FsckOptions::check()).unwrap();
    assert!(
        dirty.errors() > 0,
        "a broken descriptor in group {victim} must be reported"
    );
}

extern crate alloc;
