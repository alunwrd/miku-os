// What ext4 and JBD2 build from CRC32c
//
// The arithmetic is in `fshost::crc32c`, shared with btrfs. Here is what is
// ext4's: which bytes each checksum covers, in what order, and which chaining
// convention it uses.
//
// `crc32c_raw` is Linux's `ext4_chksum`: no inversion at either end, so a
// UUID-derived seed can feed the inode number, which feeds the inode body.
// `crc32c` is the standard form of published test vectors. They disagree for
// the same seed, and swapping them gives self-consistent checksums no other
// implementation accepts.

pub use fshost::crc32c::{crc16, crc32c, crc32c_raw, crc32c_raw_table};

/// The value other ext4 metadata checksums chain from: the UUID folded into
/// `~0`, or with `INCOMPAT_CSUM_SEED` a stored seed, so `tune2fs -U` can change
/// the UUID
pub fn ext4_csum_seed(uuid: &[u8], stored_seed: Option<u32>) -> u32 {
    match stored_seed {
        Some(s) => s,
        None => crc32c_raw(!0, uuid),
    }
}

/// Checksum of the superblock: bytes 0..1020, everything before `s_checksum`,
/// the last field at offset 0x3FC. Seeded from `~0` directly, as the UUID is in
/// the summed range
pub fn ext4_superblock_csum(sb_data: &[u8]) -> u32 {
    crc32c_raw(!0, &sb_data[..SUPERBLOCK_CSUM_OFFSET])
}

/// Where `s_checksum` lives, and how much of the superblock it covers
pub const SUPERBLOCK_CSUM_OFFSET: usize = 0x3FC;

/// Checksum of one group descriptor. The group number is folded in, so a
/// descriptor cannot move to another slot. 16 bits, the width of `bg_checksum`
pub fn ext4_group_desc_csum(seed: u32, group: u32, gd_data: &[u8]) -> u16 {
    let crc = crc32c_raw(seed, &group.to_le_bytes());
    (crc32c_raw(crc, gd_data) & 0xFFFF) as u16
}

/// Checksum of one inode, folding in the inode number and generation, so a
/// stale body does not pass after the number is reused
pub fn ext4_inode_csum(seed: u32, inode_num: u32, gen: u32, inode_data: &[u8]) -> u32 {
    let crc = crc32c_raw(seed, &inode_num.to_le_bytes());
    let crc = crc32c_raw(crc, &gen.to_le_bytes());
    crc32c_raw(crc, inode_data)
}

/// Checksum of an extent tree block, keyed by the owning inode and generation,
/// so a block reused by another file does not validate
pub fn ext4_extent_csum(seed: u32, inode_num: u32, gen: u32, extent_data: &[u8]) -> u32 {
    let crc = crc32c_raw(seed, &inode_num.to_le_bytes());
    let crc = crc32c_raw(crc, &gen.to_le_bytes());
    crc32c_raw(crc, extent_data)
}

/// Seed of every JBD2 checksum: the journal's UUID, not the filesystem's.
/// Derived, with no `tune2fs -U` to survive
pub fn jbd2_seed(journal_uuid: &[u8]) -> u32 {
    crc32c_raw(!0, journal_uuid)
}

/// Checksum of one logged block, kept in its descriptor tag. The transaction's
/// sequence number goes first, big-endian, so a stale copy from an earlier
/// transaction does not verify
pub fn jbd2_block_csum(seed: u32, sequence: u32, block: &[u8]) -> u32 {
    let crc = crc32c_raw(seed, &sequence.to_be_bytes());
    crc32c_raw(crc, block)
}

/// Checksum of a commit record, over the whole block with `h_chksum[0]`'s four
/// bytes read as zero. A torn commit does not verify, and its transaction is
/// not replayed
pub fn jbd2_commit_csum(seed: u32, commit_block: &[u8]) -> u32 {
    const ZERO: [u8; 4] = [0; 4];
    let crc = crc32c_raw(seed, &commit_block[..JBD2_COMMIT_CSUM_OFF]);
    let crc = crc32c_raw(crc, &ZERO);
    crc32c_raw(crc, &commit_block[JBD2_COMMIT_CSUM_OFF + 4..])
}

/// Checksum of the journal superblock: the 1024-byte block with its field
/// zeroed, seeded from `~0`
pub fn jbd2_superblock_csum(sb: &[u8]) -> u32 {
    const ZERO: [u8; 4] = [0; 4];
    let crc = crc32c_raw(!0, &sb[..JBD2_SB_CSUM_OFF]);
    let crc = crc32c_raw(crc, &ZERO);
    crc32c_raw(crc, &sb[JBD2_SB_CSUM_OFF + 4..])
}

/// Checksum of a descriptor or revoke block, in a four-byte tail at the block's
/// end. The field is fed in as zeros
pub fn jbd2_tail_csum(seed: u32, block: &[u8]) -> u32 {
    const ZERO: [u8; 4] = [0; 4];
    let at = block.len() - 4;
    let crc = crc32c_raw(seed, &block[..at]);
    crc32c_raw(crc, &ZERO)
}

/// `commit_header.h_chksum[0]`, past the three header words and four bytes of
/// checksum type, size, and padding
pub const JBD2_COMMIT_CSUM_OFF: usize = 16;
/// How much of a descriptor or revoke block the trailing checksum takes
pub const JBD2_TAIL_BYTES: usize = 4;
/// `journal_superblock_t.s_checksum`
pub const JBD2_SB_CSUM_OFF: usize = 252;

/// Checksum of a directory index block, `dx_root` or `dx_node`. Covers only
/// entries in use, so it changes with the count, and the eight-byte `dx_tail`
/// follows, with the checksum field as four zero bytes. `used` is the block to
/// the end of the entries in use, `tail` those eight bytes
pub fn ext4_dx_csum(seed: u32, inode_num: u32, gen: u32, used: &[u8], tail: &[u8]) -> u32 {
    const ZERO: [u8; 4] = [0; 4];
    let crc = ext4_dirent_csum(seed, inode_num, gen, used);
    let crc = crc32c_raw(crc, &tail[..4]);
    crc32c_raw(crc, &ZERO)
}

/// Checksum of a directory block, in the fake dirent at its end
pub fn ext4_dirent_csum(seed: u32, inode_num: u32, gen: u32, dir_data: &[u8]) -> u32 {
    let crc = crc32c_raw(seed, &inode_num.to_le_bytes());
    let crc = crc32c_raw(crc, &gen.to_le_bytes());
    crc32c_raw(crc, dir_data)
}

/// Checksum of a block or inode allocation bitmap
pub fn ext4_bitmap_csum(seed: u32, bitmap_data: &[u8]) -> u32 {
    crc32c_raw(seed, bitmap_data)
}

/// Legacy group descriptor checksum: CRC16 over the UUID, group number, and
/// descriptor, skipping the checksum field
pub fn ext4_group_desc_csum16(uuid: &[u8], group: u32, gd_data: &[u8]) -> u16 {
    let crc = crc16(0xFFFF, uuid);
    let crc = crc16(crc, &group.to_le_bytes());
    crc16(crc, gd_data)
}
