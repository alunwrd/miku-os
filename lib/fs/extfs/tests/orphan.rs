// The orphan list
//
// Freeing an inode is several independent writes: data blocks, block bitmap,
// inode bitmap, group counters, superblock. `s_last_orphan` names the head,
// each inode's `i_dtime` the next, and the next mount finishes the job through
// `cleanup_orphans`.

use extfs::mkfs::{mkfs, FsType, MkfsParams};
use extfs::{MemDisk, MikuFS};

const SECTORS: usize = 128 * 1024;

fn ext2_disk() -> MemDisk {
    let mut disk = MemDisk::new(SECTORS);
    let mut params = MkfsParams::new(FsType::Ext2, 0);
    params.total_sectors = SECTORS as u32;
    mkfs(&mut disk, &params).expect("mkfs failed");
    disk
}

/// A normal delete leaves the list empty: the inode goes on and off within one
/// operation
#[test]
fn a_completed_delete_leaves_the_list_empty() {
    let mut disk = ext2_disk();
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();

    let ino = fs.ext2_create_file(2, "gone.txt", 0o644).unwrap();
    fs.ext2_write_file(ino, b"contents", 0).unwrap();
    assert_eq!(fs.superblock.last_orphan(), 0);

    fs.ext2_delete_file(2, "gone.txt").unwrap();
    assert_eq!(
        fs.superblock.last_orphan(),
        0,
        "a delete that finished must not leave anything on the list"
    );
}

/// Add, then find it at the head
#[test]
fn adding_puts_the_inode_at_the_head() {
    let mut disk = ext2_disk();
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();

    let a = fs.ext2_create_file(2, "a", 0o644).unwrap();
    let b = fs.ext2_create_file(2, "b", 0o644).unwrap();

    fs.orphan_add(a).unwrap();
    assert_eq!(fs.superblock.last_orphan(), a);

    fs.orphan_add(b).unwrap();
    assert_eq!(fs.superblock.last_orphan(), b);
    // `a` is the second link, reached through b's dtime
    assert_eq!(fs.read_inode(b).unwrap().dtime(), a);
}

/// Removal works whether the inode is at the head or further down
#[test]
fn removal_handles_the_head_and_the_middle() {
    let mut disk = ext2_disk();
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();

    let a = fs.ext2_create_file(2, "a", 0o644).unwrap();
    let b = fs.ext2_create_file(2, "b", 0o644).unwrap();
    let c = fs.ext2_create_file(2, "c", 0o644).unwrap();

    fs.orphan_add(a).unwrap();
    fs.orphan_add(b).unwrap();
    fs.orphan_add(c).unwrap();
    assert_eq!(fs.superblock.last_orphan(), c);

    // From the head
    fs.orphan_del(c).unwrap();
    assert_eq!(fs.superblock.last_orphan(), b);

    // From the middle: a is behind b
    fs.orphan_del(a).unwrap();
    assert_eq!(fs.superblock.last_orphan(), b);
    assert_eq!(fs.read_inode(b).unwrap().dtime(), 0);

    fs.orphan_del(b).unwrap();
    assert_eq!(fs.superblock.last_orphan(), 0);
}

/// A delete interrupted after blocks were freed and before the inode was,
/// recovered on the next mount. Modeled by doing what `ext2_delete_file` does
/// halfway and dropping the handle, which writes nothing
#[test]
fn an_interrupted_delete_is_finished_by_the_next_mount() {
    let mut disk = ext2_disk();
    let ino;
    let free_before;

    {
        let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
        free_before = fs.superblock.free_inodes_count();

        ino = fs.ext2_create_file(2, "half.txt", 0o644).unwrap();
        let body = [0xABu8; 40_000];
        fs.ext2_write_file(ino, &body, 0).unwrap();
        fs.sync().unwrap();

        // Halfway through a delete: the name is gone, the inode is on the list,
        // and the machine stops
        fs.remove_dir_entry(2, "half.txt").unwrap();
        let mut inode = fs.read_inode(ino).unwrap();
        inode.set_links_count(0);
        fs.write_inode(ino, &inode).unwrap();
        fs.orphan_add(ino).unwrap();
        fs.sync().unwrap();

        assert_eq!(fs.superblock.last_orphan(), ino, "the record must be on disk");
    }

    // The next mount runs `activate`, where orphan cleanup lives
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
    let (_replayed, cleaned) = fs.activate().unwrap();

    assert!(cleaned >= 1, "the interrupted delete must be finished");
    assert_eq!(
        fs.superblock.last_orphan(),
        0,
        "and the list emptied afterwards"
    );
    assert_eq!(
        fs.superblock.free_inodes_count(),
        free_before,
        "the inode must be back in the pool"
    );
}

/// Freeing is idempotent, so recovery is safe: the orphan pass may free blocks
/// a partial delete already freed without inflating the free count
#[test]
fn freeing_twice_does_not_inflate_the_counters() {
    let mut disk = ext2_disk();
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();

    let blk = fs.alloc_block(0).unwrap();
    let after_alloc = fs.superblock.free_blocks_count();

    fs.free_block(blk).unwrap();
    let after_first = fs.superblock.free_blocks_count();
    assert_eq!(after_first, after_alloc + 1);

    fs.free_block(blk).unwrap();
    assert_eq!(
        fs.superblock.free_blocks_count(),
        after_first,
        "freeing an already-free block must not count twice"
    );
}

#[test]
fn freeing_an_inode_twice_does_not_inflate_the_counters() {
    let mut disk = ext2_disk();
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();

    let ino = fs.alloc_inode(0).unwrap();
    let after_alloc = fs.superblock.free_inodes_count();

    fs.free_inode(ino).unwrap();
    let after_first = fs.superblock.free_inodes_count();
    assert_eq!(after_first, after_alloc + 1);

    fs.free_inode(ino).unwrap();
    assert_eq!(fs.superblock.free_inodes_count(), after_first);
}
