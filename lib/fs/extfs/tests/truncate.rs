// Truncating to a nonzero length
//
// The size on disk moves, blocks past the new end return to the pool, the last
// kept block's tail is zeroed, and growing is sparse.

use extfs::mkfs::{mkfs, FsType, MkfsParams};
use extfs::{MemDisk, MikuFS};

const SECTORS: usize = 128 * 1024;

fn fs_of(kind: FsType) -> MemDisk {
    let mut disk = MemDisk::new(SECTORS);
    let mut params = MkfsParams::new(kind, 0);
    params.total_sectors = SECTORS as u32;
    mkfs(&mut disk, &params).expect("mkfs failed");
    disk
}

/// A file of `n` bytes, filled with a recognisable pattern
fn make_file(fs: &mut MikuFS<&mut MemDisk>, name: &str, n: usize) -> u32 {
    let ino = fs.ext3_create_file(2, name, 0o644).expect("create failed");
    let body: alloc::vec::Vec<u8> = (0..n).map(|i| (i % 251) as u8).collect();
    fs.ext3_write_file(ino, &body, 0).expect("write failed");
    ino
}

#[test]
fn shrinking_moves_the_size_on_disk() {
    let mut disk = fs_of(FsType::Ext2);
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();

    let ino = make_file(&mut fs, "shrink.bin", 40_000);
    assert_eq!(fs.read_inode(ino).unwrap().size(), 40_000);

    fs.ext3_truncate_to(ino, 5_000).unwrap();
    assert_eq!(
        fs.read_inode(ino).unwrap().size(),
        5_000,
        "the size on disk must be the new one, not the old one"
    );
}

/// Blocks past the new end come back
#[test]
fn shrinking_returns_the_blocks() {
    let mut disk = fs_of(FsType::Ext2);
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();

    let before = fs.superblock.free_blocks_count();
    let ino = make_file(&mut fs, "blocks.bin", 60_000);
    let while_full = fs.superblock.free_blocks_count();
    assert!(while_full < before, "writing the file must consume blocks");

    fs.ext3_truncate_to(ino, 1_000).unwrap();
    let after = fs.superblock.free_blocks_count();
    assert!(
        after > while_full,
        "truncating from 60000 to 1000 bytes must free blocks: {while_full} -> {after}"
    );
}

/// What survives is what was there, byte for byte
#[test]
fn the_kept_data_is_unchanged() {
    let mut disk = fs_of(FsType::Ext2);
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();

    let ino = make_file(&mut fs, "keep.bin", 30_000);
    fs.ext3_truncate_to(ino, 4_321).unwrap();

    let inode = fs.read_inode(ino).unwrap();
    let mut back = alloc::vec![0u8; 4_321];
    let n = fs.read_file(&inode, 0, &mut back).unwrap();
    assert_eq!(n, 4_321);
    for (i, b) in back.iter().enumerate() {
        assert_eq!(*b, (i % 251) as u8, "byte {i} changed");
    }
}

/// The last kept block's tail is zeroed, so growing again reads zeroes
#[test]
fn the_tail_of_the_last_block_is_zeroed() {
    let mut disk = fs_of(FsType::Ext2);
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();

    let ino = make_file(&mut fs, "tail.bin", 20_000);
    // Land mid-block on purpose
    let cut = 4_500u64;
    fs.ext3_truncate_to(ino, cut).unwrap();
    fs.ext3_truncate_to(ino, 8_000).unwrap();

    let inode = fs.read_inode(ino).unwrap();
    let mut back = alloc::vec![0u8; 8_000];
    fs.read_file(&inode, 0, &mut back).unwrap();
    for (i, b) in back.iter().enumerate().skip(cut as usize) {
        assert_eq!(*b, 0, "byte {i} past the old cut must read as zero");
    }
}

/// Growing does not allocate: the file is sparse until written
#[test]
fn growing_is_sparse() {
    let mut disk = fs_of(FsType::Ext2);
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();

    let ino = make_file(&mut fs, "grow.bin", 1_000);
    let before = fs.superblock.free_blocks_count();

    fs.ext3_truncate_to(ino, 4_000_000).unwrap();

    assert_eq!(fs.read_inode(ino).unwrap().size(), 4_000_000);
    assert_eq!(
        fs.superblock.free_blocks_count(),
        before,
        "growing by truncation must not allocate a single block"
    );
}

/// And the gap reads as zeroes
#[test]
fn a_sparse_gap_reads_as_zeroes() {
    let mut disk = fs_of(FsType::Ext2);
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();

    let ino = make_file(&mut fs, "hole.bin", 100);
    fs.ext3_truncate_to(ino, 50_000).unwrap();

    let inode = fs.read_inode(ino).unwrap();
    let mut back = alloc::vec![0u8; 50_000];
    let n = fs.read_file(&inode, 0, &mut back).unwrap();
    assert_eq!(n, 50_000);
    assert!(back[100..].iter().all(|&b| b == 0), "the gap must read as zeroes");
}

/// Truncating to zero still frees everything
#[test]
fn truncating_to_zero_still_works() {
    let mut disk = fs_of(FsType::Ext2);
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();

    let before = fs.superblock.free_blocks_count();
    let ino = make_file(&mut fs, "zero.bin", 50_000);
    fs.ext3_truncate_to(ino, 0).unwrap();

    assert_eq!(fs.read_inode(ino).unwrap().size(), 0);
    assert_eq!(
        fs.superblock.free_blocks_count(),
        before,
        "every block must come back"
    );
}

/// The same, on an extent-mapped file
#[test]
fn ext4_shrinks_too() {
    let mut disk = fs_of(FsType::Ext4);
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();

    let ino = make_file(&mut fs, "ext4.bin", 200_000);
    assert!(fs.read_inode(ino).unwrap().uses_extents());
    let while_full = fs.superblock.free_blocks_count();

    fs.ext3_truncate_to(ino, 10_000).unwrap();
    assert_eq!(fs.read_inode(ino).unwrap().size(), 10_000);
    assert!(
        fs.superblock.free_blocks_count() > while_full,
        "an extent-mapped file must return its blocks too"
    );

    let inode = fs.read_inode(ino).unwrap();
    let mut back = alloc::vec![0u8; 10_000];
    fs.read_file(&inode, 0, &mut back).unwrap();
    for (i, b) in back.iter().enumerate() {
        assert_eq!(*b, (i % 251) as u8, "byte {i} changed");
    }
}

#[test]
fn ext4_grows_sparsely() {
    let mut disk = fs_of(FsType::Ext4);
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();

    let ino = make_file(&mut fs, "ext4grow.bin", 5_000);
    let before = fs.superblock.free_blocks_count();
    fs.ext3_truncate_to(ino, 2_000_000).unwrap();

    assert_eq!(fs.read_inode(ino).unwrap().size(), 2_000_000);
    assert_eq!(fs.superblock.free_blocks_count(), before);
}

/// A truncate survives an unmount
#[test]
fn a_truncate_survives_a_remount() {
    let mut disk = fs_of(FsType::Ext2);
    let ino;
    {
        let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
        ino = make_file(&mut fs, "persist.bin", 30_000);
        fs.ext3_truncate_to(ino, 777).unwrap();
        fs.sync().unwrap();
    }
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
    assert_eq!(fs.read_inode(ino).unwrap().size(), 777);
}

extern crate alloc;

/// A read-only mount refuses a truncate and the file reads as it did
#[test]
fn a_read_only_mount_refuses_a_truncate_and_keeps_the_file() {
    for kind in [FsType::Ext2, FsType::Ext4] {
        let mut disk = fs_of(kind);
        let ino = {
            let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
            let ino = make_file(&mut fs, "kept.bin", 9_000);
            fs.sync().unwrap();
            ino
        };
        let mut fs = MikuFS::open(&mut disk, 0, true).unwrap();
        assert!(matches!(fs.ext3_truncate(ino), Err(extfs::FsError::ReadOnlyFs)), "{kind:?}");
        let inode = fs.read_inode(ino).unwrap();
        assert_eq!(inode.size(), 9_000, "{kind:?}");
        let mut back = alloc::vec![0u8; 9_000];
        assert_eq!(fs.read_file(&inode, 0, &mut back).unwrap(), 9_000);
        assert!(back.iter().enumerate().all(|(i, b)| *b == (i % 251) as u8), "{kind:?}");
    }
}
