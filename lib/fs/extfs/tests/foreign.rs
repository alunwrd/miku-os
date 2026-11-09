// Checksums against a filesystem made by e2fsprogs
//
// `tests/data/` holds the superblock, group descriptor table, and first sixteen
// inodes lifted from an image made by:
//
//     mke2fs -t ext4 -b 1024 -O metadata_csum,extent,64bit
//
// with e2fsprogs 1.47.4, whose checksums these must reproduce. The checksum
// field is fed in as zeros, not skipped, since a CRC depends on length too.

use extfs::ext4::crc32c;
use extfs::structs::Superblock;

const SB: &[u8] = include_bytes!("data/mke2fs-metadata-csum.bin");
const GDT: &[u8] = include_bytes!("data/mke2fs-gdt.bin");
const INODES: &[u8] = include_bytes!("data/mke2fs-inodes.bin");

const INODE_SIZE: usize = 256;
const GOOD_OLD_INODE_SIZE: usize = 128;

fn superblock() -> Superblock {
    let mut sb = Superblock::zeroed();
    sb.data.copy_from_slice(SB);
    sb
}

#[test]
fn the_reference_image_is_what_we_think_it_is() {
    let sb = superblock();
    assert_eq!(sb.magic(), 0xEF53);
    assert!(sb.has_metadata_csum(), "the sample must use metadata_csum");
    assert!(sb.has_csum_seed(), "and carry its own checksum seed");
    assert_eq!(sb.checksum_type(), 1, "crc32c");
    assert_eq!(sb.group_desc_size(), 64, "64bit descriptors");
    assert_eq!(sb.inode_size_val(), INODE_SIZE as u32);
}

/// The superblock's checksum from e2fsprogs, confirming the offset (0x3FC) and
/// accumulation: seeded with ~0, never inverted
#[test]
fn we_reproduce_the_superblock_checksum() {
    let sb = superblock();
    let stored = sb.checksum();
    let computed = crc32c::ext4_superblock_csum(&sb.data);
    assert_eq!(
        computed, stored,
        "e2fsprogs says {stored:#010x}, we say {computed:#010x}"
    );
}

/// The seed other checksums chain from. On a fresh filesystem the stored seed
/// and the UUID's derivation match, since `mke2fs` writes the derivation; they
/// diverge only after `tune2fs -U`. So this checks the derivation
#[test]
fn our_seed_derivation_matches_e2fsprogs() {
    let sb = superblock();
    let stored = sb.checksum_seed();
    let derived = crc32c::ext4_csum_seed(sb.uuid(), None);
    assert_eq!(
        derived, stored,
        "mke2fs seeded this volume from its UUID; our derivation says \
         {derived:#010x} and the field says {stored:#010x}"
    );
    // A stored seed is used verbatim
    assert_eq!(crc32c::ext4_csum_seed(sb.uuid(), Some(0xDEAD_BEEF)), 0xDEAD_BEEF);
}

/// Group descriptors: the checksum field is replaced by two zero bytes
#[test]
fn we_reproduce_the_group_descriptor_checksums() {
    let sb = superblock();
    let seed = sb.checksum_seed();
    let ds = sb.group_desc_size() as usize;

    let mut checked = 0;
    for group in 0..(GDT.len() / ds).min(8) {
        let d = &GDT[group * ds..(group + 1) * ds];
        let stored = u16::from_le_bytes([d[30], d[31]]);
        if d.iter().all(|&b| b == 0) {
            continue;
        }

        let c = crc32c::crc32c_raw(seed, &(group as u32).to_le_bytes());
        let c = crc32c::crc32c_raw(c, &d[..30]);
        let c = crc32c::crc32c_raw(c, &[0, 0]);
        let c = crc32c::crc32c_raw(c, &d[32..ds]);

        assert_eq!(
            (c & 0xFFFF) as u16,
            stored,
            "group {group}: e2fsprogs says {stored:#06x}, we say {:#06x}",
            (c & 0xFFFF) as u16
        );
        checked += 1;
    }
    assert!(checked >= 3, "only {checked} descriptors were checked");
}

/// Inodes: both checksum halves are fed as zeros. The field at 124 is inside
/// the classic 128-byte inode, the one at 130 in the extra area
#[test]
fn we_reproduce_the_inode_checksums() {
    let sb = superblock();
    let seed = sb.checksum_seed();

    let mut checked = 0;
    for ino in 1..=(INODES.len() / INODE_SIZE) as u32 {
        let raw = &INODES[(ino as usize - 1) * INODE_SIZE..ino as usize * INODE_SIZE];
        let mode = u16::from_le_bytes([raw[0], raw[1]]);
        if mode == 0 {
            continue; // never used, nothing to check
        }

        let lo = u16::from_le_bytes([raw[124], raw[125]]);
        let hi = u16::from_le_bytes([raw[130], raw[131]]);
        let generation = u32::from_le_bytes([raw[100], raw[101], raw[102], raw[103]]);
        let extra = u16::from_le_bytes([raw[128], raw[129]]) as usize;
        let has_hi = GOOD_OLD_INODE_SIZE + extra >= 132;

        let c = crc32c::crc32c_raw(seed, &ino.to_le_bytes());
        let c = crc32c::crc32c_raw(c, &generation.to_le_bytes());
        let c = crc32c::crc32c_raw(c, &raw[..124]);
        let c = crc32c::crc32c_raw(c, &[0, 0]);
        let c = crc32c::crc32c_raw(c, &raw[126..GOOD_OLD_INODE_SIZE]);
        let c = crc32c::crc32c_raw(c, &raw[GOOD_OLD_INODE_SIZE..130]);
        let c = if has_hi {
            let c = crc32c::crc32c_raw(c, &[0, 0]);
            crc32c::crc32c_raw(c, &raw[132..INODE_SIZE])
        } else {
            crc32c::crc32c_raw(c, &raw[130..INODE_SIZE])
        };

        let stored = if has_hi { (lo as u32) | ((hi as u32) << 16) } else { lo as u32 };
        let got = if has_hi { c } else { c & 0xFFFF };
        assert_eq!(
            got, stored,
            "inode {ino}: e2fsprogs says {stored:#010x}, we say {got:#010x}"
        );
        checked += 1;
    }
    assert!(checked >= 3, "only {checked} inodes were checked");
}

/// The driver's routines agree with the reference above, so the checks in
/// `MikuFS` are the ones validated
#[test]
fn the_driver_agrees_with_the_reference() {
    use extfs::mkfs::{mkfs, FsType, MkfsParams};
    use extfs::{MemDisk, MikuFS};

    let mut disk = MemDisk::new(128 * 1024);
    let mut params = MkfsParams::new(FsType::Ext4, 0);
    params.total_sectors = 128 * 1024;
    mkfs(&mut disk, &params).unwrap();
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();

    // Grafts in the reference superblock, so the driver uses the vectors'
    // fields
    fs.superblock.data.copy_from_slice(SB);
    assert!(fs.verify_superblock_csum(), "the driver must accept a real superblock");
    assert_eq!(fs.csum_seed(), fs.superblock.checksum_seed());

    let ds = fs.superblock.group_desc_size() as usize;
    fs.groups[0].data[..ds].copy_from_slice(&GDT[..ds]);
    assert!(
        fs.verify_group_desc_csum(0),
        "the driver must accept a real group descriptor"
    );
}

// Extents
//
// A second image with 2 KiB blocks:
//
//     mke2fs -t ext4 -b 2048 -I 256 -O metadata_csum,extent,64bit
//
// fragmented with debugfs (write a hundred files, delete every other one, write
// a large one into the holes), so the file needs an extent tree block.
//
// For 1 KiB and 4 KiB blocks the extent tail is the block's last four bytes,
// the same address as right past the last entry. For 2 KiB they are four bytes
// apart, so this sample can tell them apart.

const EXT_SB: &[u8] = include_bytes!("data/mke2fs-extent-sb.bin");
const EXT_NODE: &[u8] = include_bytes!("data/mke2fs-extent-node.bin");

/// Inode 12, generation 0: the file debugfs wrote into the fragmented image
const EXT_NODE_INO: u32 = 12;
const EXT_NODE_GEN: u32 = 0;

fn extent_superblock() -> Superblock {
    let mut sb = Superblock::zeroed();
    sb.data.copy_from_slice(EXT_SB);
    sb
}

fn eh_max(node: &[u8]) -> usize {
    u16::from_le_bytes([node[4], node[5]]) as usize
}

/// `eh_max` is not reduced for the checksum: the four bytes come from what the
/// header and entries leave, at least four for every legal block size
#[test]
fn a_checksummed_node_holds_the_same_number_of_extents() {
    assert_eq!(EXT_NODE.len(), 2048);
    assert_eq!(eh_max(EXT_NODE), (2048 - 12) / 12);
}

/// The tail is right past the last entry the header admits, not at the block's
/// end. On this sample they differ
#[test]
fn the_tail_follows_the_entries_not_the_block() {
    let at = 12 + eh_max(EXT_NODE) * 12;
    assert_eq!(at, 2040);
    assert_ne!(at, EXT_NODE.len() - 4, "the sample must be able to tell them apart");
    // and the four bytes the layout leaves over are untouched
    assert_eq!(&EXT_NODE[2044..2048], &[0, 0, 0, 0]);
}

/// The checksum e2fsprogs put in that tail, reproduced from our own crc32c
#[test]
fn we_reproduce_a_real_extent_block_checksum() {
    let sb = extent_superblock();
    assert!(sb.has_metadata_csum());
    let seed = crc32c::ext4_csum_seed(sb.uuid(), Some(sb.checksum_seed()));

    let at = 12 + eh_max(EXT_NODE) * 12;
    let stored = u32::from_le_bytes([
        EXT_NODE[at],
        EXT_NODE[at + 1],
        EXT_NODE[at + 2],
        EXT_NODE[at + 3],
    ]);
    let computed = crc32c::ext4_extent_csum(seed, EXT_NODE_INO, EXT_NODE_GEN, &EXT_NODE[..at]);
    assert_eq!(
        computed, stored,
        "e2fsprogs says {stored:#010x}, we say {computed:#010x}"
    );
}

/// The same bytes summed to the block's end, which must not match
#[test]
fn summing_to_the_end_of_the_block_gives_a_different_answer() {
    let sb = extent_superblock();
    let seed = crc32c::ext4_csum_seed(sb.uuid(), Some(sb.checksum_seed()));
    let at = 12 + eh_max(EXT_NODE) * 12;
    let right = crc32c::ext4_extent_csum(seed, EXT_NODE_INO, EXT_NODE_GEN, &EXT_NODE[..at]);
    let wrong = crc32c::ext4_extent_csum(
        seed,
        EXT_NODE_INO,
        EXT_NODE_GEN,
        &EXT_NODE[..EXT_NODE.len() - 4],
    );
    assert_ne!(right, wrong);
}

/// And the driver's own reader agrees, through the same path a mount uses
#[test]
fn the_driver_verifies_a_real_extent_block() {
    use extfs::mkfs::{mkfs, FsType, MkfsParams};
    use extfs::{MemDisk, MikuFS};

    let mut disk = MemDisk::new(128 * 1024);
    let mut params = MkfsParams::new(FsType::Ext4, 0);
    params.total_sectors = 128 * 1024;
    mkfs(&mut disk, &params).unwrap();
    let mut fs = MikuFS::open(&mut disk, 0, false).unwrap();

    fs.superblock.data.copy_from_slice(EXT_SB);
    fs.block_size = 2048;
    assert!(
        fs.verify_extent_block(EXT_NODE_INO, EXT_NODE_GEN, EXT_NODE),
        "the driver must accept a real extent tree block"
    );

    // A single flipped bit in the entries has to be caught
    let mut damaged = [0u8; 2048];
    damaged.copy_from_slice(EXT_NODE);
    damaged[20] ^= 0x01;
    assert!(!fs.verify_extent_block(EXT_NODE_INO, EXT_NODE_GEN, &damaged));

    // So does the node under the wrong inode, keeping a reused block from
    // validating
    assert!(!fs.verify_extent_block(EXT_NODE_INO + 1, EXT_NODE_GEN, EXT_NODE));
}

// the JBD2 journal

/// Five blocks from a journal e2fsprogs wrote with checksums on: the journal
/// superblock, a descriptor, the two logged blocks, and the commit record. Made
/// with
///
///     mke2fs -t ext4 -b 1024 -I 256
///     # s_feature_incompat |= CSUM_V3, s_checksum_type = 4, superblock
///     # checksum recomputed, which `journal_open` then accepts
///     debugfs -w -f cmds     # journal_open; journal_write -b 100,101 payload
///
/// Tags here are sixteen bytes, not the eight of ordinary `mke2fs` journals
const JBD2: &[u8] = include_bytes!("data/mke2fs-jbd2-csumv3.bin");
const JBD2_BS: usize = 1024;

fn jblock(n: usize) -> &'static [u8] {
    &JBD2[n * JBD2_BS..(n + 1) * JBD2_BS]
}

fn journal_superblock() -> extfs::ext3::journal::JournalSuperblock {
    let mut jsb = extfs::ext3::journal::JournalSuperblock::zeroed();
    jsb.data.copy_from_slice(jblock(0));
    jsb
}

#[test]
fn the_reference_journal_is_what_we_think_it_is() {
    let jsb = journal_superblock();
    assert!(jsb.is_valid(), "journal superblock magic");
    assert!(jsb.is_v2());
    assert_eq!(jsb.blocksize(), JBD2_BS as u32);
    assert!(jsb.is_csum_v3(), "the sample must carry CSUM_V3");
    assert_eq!(jsb.checksum_type(), 4, "crc32c, which is what JBD2 numbers 4");
    assert_eq!(jsb.tag_size(), 16, "a v3 tag is four big-endian words");
    assert_eq!(jsb.unsupported_incompat(), 0, "and nothing else we refuse");

    // Descriptor at journal block 1, two tags. The first has no SAME_UUID, so
    // sixteen bytes of UUID follow, putting the second at 12 + 16 + 16
    let desc = jblock(1);
    assert_eq!(u32::from_be_bytes([desc[4], desc[5], desc[6], desc[7]]), 1, "descriptor");
    assert_eq!(u32::from_be_bytes([desc[12], desc[13], desc[14], desc[15]]), 100);
    assert_eq!(jsb.tag_flags(desc, 12), 0x0);
    assert_eq!(u32::from_be_bytes([desc[44], desc[45], desc[46], desc[47]]), 101);
    assert_eq!(jsb.tag_flags(desc, 44), 0xa, "SAME_UUID | LAST_TAG");

    let commit = jblock(4);
    assert_eq!(u32::from_be_bytes([commit[4], commit[5], commit[6], commit[7]]), 2);
}

/// An eight-byte stride would read the second tag from the first one's UUID
#[test]
fn the_old_stride_reads_the_wrong_block_number() {
    let desc = jblock(1);
    let wrong = u32::from_be_bytes([desc[20], desc[21], desc[22], desc[23]]);
    assert_ne!(wrong, 101, "the sample must be able to show the difference");
}

#[test]
fn we_reproduce_the_journal_superblock_checksum() {
    let sb = jblock(0);
    let stored = u32::from_be_bytes([sb[252], sb[253], sb[254], sb[255]]);
    let computed = crc32c::jbd2_superblock_csum(sb);
    assert_eq!(computed, stored, "e2fsprogs says {stored:#010x}, we say {computed:#010x}");
}

#[test]
fn we_reproduce_the_logged_block_checksums() {
    let jsb = journal_superblock();
    let seed = crc32c::jbd2_seed(jsb.uuid());
    let desc = jblock(1);
    let sequence = u32::from_be_bytes([desc[8], desc[9], desc[10], desc[11]]);

    for (tag_at, data_block) in [(12usize, 2usize), (44, 3)] {
        let stored = jsb.tag_checksum(desc, tag_at);
        let computed = crc32c::jbd2_block_csum(seed, sequence, jblock(data_block));
        assert_eq!(
            computed, stored,
            "tag at {tag_at}: e2fsprogs says {stored:#010x}, we say {computed:#010x}"
        );
    }
}

#[test]
fn we_reproduce_the_commit_record_checksum() {
    let jsb = journal_superblock();
    let seed = crc32c::jbd2_seed(jsb.uuid());
    let commit = jblock(4);
    let stored = u32::from_be_bytes([
        commit[crc32c::JBD2_COMMIT_CSUM_OFF],
        commit[crc32c::JBD2_COMMIT_CSUM_OFF + 1],
        commit[crc32c::JBD2_COMMIT_CSUM_OFF + 2],
        commit[crc32c::JBD2_COMMIT_CSUM_OFF + 3],
    ]);
    let computed = crc32c::jbd2_commit_csum(seed, commit);
    assert_eq!(computed, stored, "e2fsprogs says {stored:#010x}, we say {computed:#010x}");
}

/// The checksum field is fed in as zeros, with the same negative control
#[test]
fn skipping_the_commit_checksum_field_gives_a_different_answer() {
    let jsb = journal_superblock();
    let seed = crc32c::jbd2_seed(jsb.uuid());
    let commit = jblock(4);
    let right = crc32c::jbd2_commit_csum(seed, commit);
    let at = crc32c::JBD2_COMMIT_CSUM_OFF;
    let skipped = crc32c::crc32c_raw(
        crc32c::crc32c_raw(seed, &commit[..at]),
        &commit[at + 4..],
    );
    assert_ne!(right, skipped);
}
