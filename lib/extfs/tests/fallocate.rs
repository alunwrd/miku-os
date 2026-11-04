// Unwritten extents
//
// `fallocate` reserves blocks that read as zeros. ext4's way is a bit in
// `ee_len`: such an extent holds nothing, every reader treats it as a hole, and
// the first write inside splits it so only that block becomes written.
//
// Checked here: nothing is written at reservation, and everything reads
// correctly after.

use extfs::mkfs::{mkfs, FsType, MkfsParams};
use extfs::{MemDisk, MikuFS};

const SECTORS: usize = 128 * 1024;

fn ext4() -> MemDisk {
    let mut disk = MemDisk::new(SECTORS);
    let mut params = MkfsParams::new(FsType::Ext4, 0);
    params.total_sectors = SECTORS as u32;
    params.block_size = 1024;
    mkfs(&mut disk, &params).expect("mkfs failed");
    disk
}

#[test]
fn reserving_space_writes_almost_nothing() {
    let mut disk = ext4();
    let writes_for_data;
    let writes_for_reservation;

    {
        let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
        let ino = fs.ext4_create_file(2, "reserved.bin", 0o644).unwrap();
        let before = fs.reader.host.writes;
        fs.ext4_fallocate(ino, 0, 256 * 1024).expect("fallocate failed");
        fs.sync().unwrap();
        writes_for_reservation = fs.reader.host.writes - before;
    }

    let mut disk2 = ext4();
    {
        let mut fs = MikuFS::open(&mut disk2, 0, false).unwrap();
        let ino = fs.ext4_create_file(2, "written.bin", 0o644).unwrap();
        let before = fs.reader.host.writes;
        let data = vec![0u8; 256 * 1024];
        fs.ext4_write_file(ino, &data, 0).expect("write failed");
        fs.sync().unwrap();
        writes_for_data = fs.reader.host.writes - before;
    }

    assert!(
        writes_for_reservation * 4 < writes_for_data,
        "reserving 256 KiB cost {writes_for_reservation} writes against \
         {writes_for_data} for writing it - fallocate is still writing the data"
    );
}

#[test]
fn reserved_space_reads_as_zeroes() {
    let mut disk = ext4();
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
    let bs = fs.block_size as usize;

    // Puts a recognizable pattern on the blocks about to be handed out, so a
    // read reaching the disk cannot pass by accident
    let scratch = fs.ext4_create_file(2, "scratch.bin", 0o644).unwrap();
    let junk = vec![0xA5u8; 64 * 1024];
    fs.ext4_write_file(scratch, &junk, 0).unwrap();
    fs.ext4_delete_file(2, "scratch.bin").unwrap();

    let ino = fs.ext4_create_file(2, "reserved.bin", 0o644).unwrap();
    fs.ext4_fallocate(ino, 0, 64 * 1024).unwrap();

    let inode = fs.read_inode(ino).unwrap();
    assert_eq!(inode.size(), 64 * 1024);
    let mut buf = vec![0xFFu8; bs];
    for round in 0..(64 * 1024 / bs) as u64 {
        let n = fs.read_file(&inode, round * bs as u64, &mut buf).unwrap();
        assert_eq!(n, bs);
        assert!(
            buf.iter().all(|&b| b == 0),
            "block {round} of reserved space did not read as zeros"
        );
    }
}

#[test]
fn the_blocks_really_are_reserved() {
    let mut disk = ext4();
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();

    let free_before = fs.superblock.free_blocks_count();
    let ino = fs.ext4_create_file(2, "reserved.bin", 0o644).unwrap();
    fs.ext4_fallocate(ino, 0, 64 * 1024).unwrap();
    let free_after = fs.superblock.free_blocks_count();

    let bs = fs.block_size;
    let reserved = free_before - free_after;
    assert!(
        reserved >= 64 * 1024 / bs,
        "fallocate took {reserved} blocks for {} - it did not reserve them",
        64 * 1024 / bs
    );

    // And the inode agrees about how much space it holds
    let inode = fs.read_inode(ino).unwrap();
    assert!(inode.blocks_full() >= (64 * 1024 / 512) as u64);
}

#[test]
fn writing_into_reserved_space_keeps_the_rest_zero() {
    let mut disk = ext4();
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
    let bs = fs.block_size as usize;

    let ino = fs.ext4_create_file(2, "reserved.bin", 0o644).unwrap();
    fs.ext4_fallocate(ino, 0, 64 * 1024).unwrap();

    // Write in the middle, which splits the extent three ways
    let body = vec![0x5Au8; bs];
    let at = 20 * bs as u64;
    fs.ext4_write_file(ino, &body, at).unwrap();

    let inode = fs.read_inode(ino).unwrap();
    let mut buf = vec![0xFFu8; bs];
    for round in 0..(64 * 1024 / bs) as u64 {
        fs.read_file(&inode, round * bs as u64, &mut buf).unwrap();
        if round == 20 {
            assert!(buf.iter().all(|&b| b == 0x5A), "the written block is wrong");
        } else {
            assert!(
                buf.iter().all(|&b| b == 0),
                "block {round} stopped reading as zeros after a write elsewhere"
            );
        }
    }
}

/// The split survives reading back from disk
#[test]
fn a_split_reservation_survives_a_remount() {
    let mut disk = ext4();
    let ino;
    let bs;
    {
        let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
        bs = fs.block_size as usize;
        ino = fs.ext4_create_file(2, "reserved.bin", 0o644).unwrap();
        fs.ext4_fallocate(ino, 0, 64 * 1024).unwrap();
        for round in [3u64, 20, 40] {
            let body = vec![round as u8; bs];
            fs.ext4_write_file(ino, &body, round * bs as u64).unwrap();
        }
        fs.sync().unwrap();
    }

    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
    let inode = fs.read_inode(ino).unwrap();
    let mut buf = vec![0xFFu8; bs];
    for round in 0..(64 * 1024 / bs) as u64 {
        fs.read_file(&inode, round * bs as u64, &mut buf).unwrap();
        let want = if [3, 20, 40].contains(&round) { round as u8 } else { 0 };
        assert!(
            buf.iter().all(|&b| b == want),
            "block {round} reads {:#04x}, expected {want:#04x}",
            buf[0]
        );
    }
}

/// Reserved blocks are still this file's, so truncating past them returns them
#[test]
fn truncating_returns_reserved_blocks() {
    let mut disk = ext4();
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();

    let free_before = fs.superblock.free_blocks_count();
    let ino = fs.ext4_create_file(2, "reserved.bin", 0o644).unwrap();
    fs.ext4_fallocate(ino, 0, 64 * 1024).unwrap();
    fs.ext4_truncate_to(ino, 0).unwrap();
    let free_after = fs.superblock.free_blocks_count();

    assert!(
        free_after + 4 >= free_before,
        "truncating a reservation left {} blocks unaccounted for",
        free_before - free_after
    );
}
