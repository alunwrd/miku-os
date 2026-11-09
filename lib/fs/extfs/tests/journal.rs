// The journal against a disk that can lose data
//
// `MemDisk` models a device with a volatile write cache: ordinary writes are
// staged, write-through and barrier writes bypass the stage, `flush` commits
// it, and `crash()` discards it.
//
// Under test is the write-ahead rule: a change may not reach its real location
// before its log record is durable.

use extfs::mkfs::{mkfs, FsType, MkfsParams};
use extfs::{MemDisk, MikuFS};

const SECTORS: usize = 128 * 1024;

fn ext3_disk() -> MemDisk {
    let mut disk = MemDisk::new(SECTORS);
    let mut params = MkfsParams::new(FsType::Ext3, 0);
    params.total_sectors = SECTORS as u32;
    mkfs(&mut disk, &params).expect("mkfs failed");
    disk
}

#[test]
fn ext3_mounts_with_a_live_journal() {
    let mut disk = ext3_disk();
    let fs = MikuFS::open(&mut disk, 0, false).unwrap();
    assert!(fs.journal_active, "ext3 must come up with the journal engaged");
    assert!(fs.journal_maxlen >= 4);
    assert!(fs.journal_first > 0 && fs.journal_first < fs.journal_maxlen);
}

/// A journal superblock claiming more blocks than the inode holds would walk
/// replay off its end
#[test]
fn an_impossible_journal_superblock_is_refused() {
    let mut disk = ext3_disk();
    {
        let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
        // s_maxlen at byte 16 of the journal superblock, big-endian
        let jblk = fs.journal_block_to_disk(0).unwrap();
        let bs = fs.block_size as usize;
        let mut buf = [0u8; 4096];
        fs.read_block_into(jblk, &mut buf[..bs]).unwrap();
        buf[16..20].copy_from_slice(&0x00FF_FFFFu32.to_be_bytes());
        fs.write_block_data_direct(jblk, &buf[..bs]).unwrap();
        fs.sync().unwrap();
    }
    let fs = MikuFS::open(&mut disk, 0, false).unwrap();
    assert!(
        !fs.journal_active,
        "a journal longer than its own inode must not be believed"
    );
}

// Handles and batching

/// `begin`/`commit` are handle open and close, and they nest, so a compound
/// operation is one transaction
#[test]
fn handles_nest() {
    let mut disk = ext3_disk();
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();

    fs.ext3_begin_txn().unwrap();
    assert!(fs.txn_active());
    let seq = fs.running.as_ref().unwrap().sequence;

    fs.ext3_begin_txn().unwrap();
    fs.ext3_commit_txn().unwrap();
    assert!(fs.txn_active(), "the inner close must not end the transaction");
    assert_eq!(fs.running.as_ref().unwrap().sequence, seq);

    fs.ext3_commit_txn().unwrap();
}

/// The tag list grows past 64 entries
#[test]
fn a_transaction_records_more_than_64_blocks() {
    let mut disk = ext3_disk();
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();

    fs.ext3_begin_txn().unwrap();
    let first = fs.superblock.first_data_block();
    for i in 0..100u32 {
        fs.ext3_journal_current_block(first + 10 + i).unwrap();
    }
    let logged = fs.running.as_ref().unwrap().tags.len();
    assert!(
        logged >= 100,
        "only {logged} of 100 blocks were logged - the tag list is still capped"
    );
}

/// Logging a block twice in one transaction records it once
#[test]
fn logging_a_block_twice_records_it_once() {
    let mut disk = ext3_disk();
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
    fs.ext3_begin_txn().unwrap();
    let blk = fs.superblock.first_data_block() + 20;
    fs.ext3_journal_current_block(blk).unwrap();
    fs.ext3_journal_current_block(blk).unwrap();
    assert_eq!(fs.running.as_ref().unwrap().tags.len(), 1);
}

// The write-ahead rule

/// A logged block is pinned and not offered for checkpointing:
/// `get_dirty_blocks` is what checkpoints iterate
#[test]
fn a_logged_block_is_not_offered_for_checkpoint() {
    let mut disk = ext3_disk();
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();

    let bs = fs.block_size as usize;
    let blk = fs.alloc_block(0).unwrap();

    fs.ext3_begin_txn().unwrap();
    fs.ext3_journal_current_block(blk).unwrap();
    fs.write_block_data(blk, &[0xAB; 4096][..bs]).unwrap();

    let cache = fs.block_cache.as_ref().unwrap();
    assert!(cache.is_pinned(blk), "a logged block must be pinned");
    assert!(
        !cache.get_dirty_blocks().iter().any(|&(b, _)| b == blk),
        "a pinned block must not be handed to a checkpoint"
    );
    assert!(
        cache.all_dirty_blocks().iter().any(|&(b, _)| b == blk),
        "it is still dirty, just not checkpointable yet"
    );
}

/// And the commit releases it
#[test]
fn committing_releases_the_pin() {
    let mut disk = ext3_disk();
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();

    let bs = fs.block_size as usize;
    let blk = fs.alloc_block(0).unwrap();

    fs.ext3_begin_txn().unwrap();
    fs.ext3_journal_current_block(blk).unwrap();
    fs.write_block_data(blk, &[0xCD; 4096][..bs]).unwrap();
    assert!(fs.block_cache.as_ref().unwrap().is_pinned(blk));

    fs.force_commit().unwrap();
    assert!(
        !fs.block_cache.as_ref().unwrap().is_pinned(blk),
        "the commit record is durable, so the block may now be checkpointed"
    );
}

/// The ordering observed on the medium. A change in an open transaction, cache
/// pressure for its slot, then the filesystem dropped unsynced, as a crash. The
/// real location must hold the old contents
#[test]
fn nothing_reaches_the_real_location_before_the_commit() {
    let mut disk = ext3_disk();
    let blk;
    let bs;
    {
        let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
        bs = fs.block_size as usize;
        blk = fs.alloc_block(0).unwrap();
        fs.write_block_data(blk, &[0x11; 4096][..bs]).unwrap();
        fs.sync().unwrap();

        fs.ext3_begin_txn().unwrap();
        fs.ext3_journal_current_block(blk).unwrap();
        fs.write_block_data(blk, &[0xEE; 4096][..bs]).unwrap();

        // While the transaction is open, its logged block stays pinned
        assert!(
            !fs.txn_active() || fs.block_cache.as_ref().unwrap().is_pinned(blk),
            "an open transaction must keep its logged blocks pinned"
        );

        // Makes the cache want that slot
        for i in 0..400u32 {
            let Ok(other) = fs.alloc_block(0) else { break };
            if other != blk {
                let _ = fs.write_block_data(other, &[i as u8; 4096][..bs]);
            }
        }

        assert!(
            !fs.txn_active() || fs.block_cache.as_ref().unwrap().is_pinned(blk),
            "cache pressure must not be resolved by evicting a pinned block"
        );
    }

    // The crash: the handle is gone, nothing else written
    let spb = bs / 512;
    let start = blk as usize * spb * 512;
    let image = disk.image();
    let landed = &image[start..start + bs];
    assert!(
        landed.iter().all(|&b| b != 0xEE) || landed.iter().any(|&b| b == 0x11),
        "the uncommitted contents reached the block's real location"
    );
    assert_eq!(
        landed[0], 0x11,
        "the block must still hold what was there before the open transaction"
    );
}

// Replay

/// A journal larger than 512 blocks replays in full, as a real mkfs.ext3 makes
/// them
#[test]
fn a_journal_larger_than_512_blocks_is_scanned_whole() {
    let mut disk = MemDisk::new(SECTORS);
    let mut params = MkfsParams::new(FsType::Ext3, 0);
    params.total_sectors = SECTORS as u32;
    params.journal_blocks = 1024;
    mkfs(&mut disk, &params).expect("mkfs failed");

    let fs = MikuFS::open(&mut disk, 0, false).unwrap();
    assert!(
        fs.journal_maxlen > 512,
        "test needs a journal past the old scan limit, got {}",
        fs.journal_maxlen
    );
}

/// Sequence numbers wrap; a plain `a < b` would skip the whole remaining log
/// after a wrap
#[test]
fn sequence_comparison_survives_a_wrap() {
    use extfs::ext3::recovery::seq_before;

    assert!(seq_before(5, 9));
    assert!(!seq_before(9, 5));
    assert!(!seq_before(7, 7));

    // Across the wrap: u32::MAX - 1 precedes 3
    assert!(seq_before(u32::MAX - 1, 3));
    assert!(!seq_before(3, u32::MAX - 1));
}

/// An operation surviving a clean unmount is there on the next mount, the
/// baseline for crash cases
#[test]
fn a_committed_change_survives_a_remount() {
    let mut disk = ext3_disk();
    let ino;
    {
        let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
        ino = fs.ext3_create_file(2, "durable.txt", 0o644).unwrap();
        fs.ext3_write_file(ino, b"written before the unmount", 0).unwrap();
        fs.sync().unwrap();
    }
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
    let found = fs.ext2_lookup_in_dir(2, "durable.txt").unwrap();
    assert_eq!(found, Some(ino), "a synced file must be there after remount");
}

/// What a create costs the log: the directory block with the entry, the inode
/// table block, and two bitmaps, not one per directory block
#[test]
fn a_create_logs_a_handful_of_blocks() {
    let mut disk = ext3_disk();
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();
    fs.activate().unwrap();
    assert!(fs.journal_active);

    let dir = fs.ext3_create_dir(2, "d", 0o755).unwrap();
    for i in 0..300 {
        fs.ext3_create_file(dir, &format!("f{i:04}"), 0o644).unwrap();
    }
    fs.sync().unwrap();
    let inode = fs.read_inode(dir).unwrap();
    let dir_blocks = inode.size() / fs.block_size as u64;
    assert!(dir_blocks > 4, "the directory has to be several blocks by now");

    // One more, and count what the transaction picks up
    fs.ext3_begin_txn().unwrap();
    fs.ext2_create_file(dir, "counted", 0o644).unwrap();
    let logged = fs.running.as_ref().map(|t| t.tags.len()).unwrap_or(0);
    fs.ext3_commit_txn().unwrap();
    fs.sync().unwrap();

    eprintln!("one create logged {logged} blocks in a {dir_blocks}-block directory");
    assert!(logged > 0, "a create with nothing logged is the old bug");
    assert!(
        (logged as u64) < dir_blocks,
        "logged {logged} blocks for one entry in a {dir_blocks}-block directory"
    );
    assert!(logged <= 8, "logged {logged} blocks for a single create");
}
