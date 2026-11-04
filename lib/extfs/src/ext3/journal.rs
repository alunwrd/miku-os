use crate::structs::*;
use crate::{FsError, MikuFS};
use crate::host::ExtHost;

pub const JBD_MAGIC: u32 = 0xC03B3998;

pub const JBD_DESCRIPTOR_BLOCK: u32 = 1;
pub const JBD_COMMIT_BLOCK: u32 = 2;
pub const JBD_SUPERBLOCK_V1: u32 = 3;
pub const JBD_SUPERBLOCK_V2: u32 = 4;
pub const JBD_REVOKE_BLOCK: u32 = 5;

pub const JBD_FLAG_ESCAPE: u32 = 1;
pub const JBD_FLAG_SAME_UUID: u32 = 2;
pub const JBD_FLAG_DELETED: u32 = 4;
pub const JBD_FLAG_LAST_TAG: u32 = 8;

pub const DEFAULT_JOURNAL_BLOCKS: u32 = 256;

/// How long a transaction stays open collecting operations. Linux's five
/// seconds
pub const COMMIT_INTERVAL_SECS: u64 = 5;

#[derive(Clone, Copy)]
pub struct TxnTag {
    pub fs_block: u32,
    pub journal_pos: u32,
}

impl TxnTag {
    pub const fn empty() -> Self {
        Self { fs_block: 0, journal_pos: 0 }
    }
}

/// One transaction's logged state. Both lists grow;
/// `ext3_journal_current_block` forces a commit when a transaction outgrows the
/// journal, since a larger one could never be written
pub struct Transaction {
    /// The sequence number this transaction will commit under
    pub sequence: u32,
    /// Journal block holding the descriptor record
    pub desc_pos: u32,
    /// Blocks logged, with the journal slot reserved for each
    pub tags: alloc::vec::Vec<TxnTag>,
    /// Blocks whose older logged copies must not be replayed: everything this
    /// transaction freed. A set, since a big file frees tens of thousands, each
    /// checked
    pub revokes: alloc::collections::BTreeSet<u32>,
    /// Every block this transaction freed, metadata or data, as runs (first
    /// block to length). Until commit a crash returns them to old owners, so
    /// they are not reused (`freed_in_running`)
    pub freed: alloc::collections::BTreeMap<u32, u32>,
    /// Open `begin`/`commit` pairs. A commit happens only at zero, so a
    /// compound operation is one transaction
    pub handles: u32,
}

impl Transaction {
    pub fn new(sequence: u32, desc_pos: u32) -> Self {
        Self {
            sequence,
            desc_pos,
            tags: alloc::vec::Vec::new(),
            revokes: alloc::collections::BTreeSet::new(),
            freed: alloc::collections::BTreeMap::new(),
            handles: 0,
        }
    }

    /// Journal blocks this transaction will occupy: the descriptor, one per
    /// logged block, the revoke record if any, and the commit
    pub fn journal_blocks(&self) -> u32 {
        let revoke = if self.revokes.is_empty() { 0 } else { 1 };
        1 + self.tags.len() as u32 + revoke + 1
    }
}

/// JBD2 journal features. `REVOKE` is written and read; `64BIT` and `CSUM_V3`
/// change a descriptor tag's size and can be walked; `ASYNC_COMMIT` and
/// `FAST_COMMIT` change what a valid log is, so a journal with either is left
/// alone
pub const JBD_INCOMPAT_REVOKE: u32 = 0x0001;
pub const JBD_INCOMPAT_64BIT: u32 = 0x0002;
pub const JBD_INCOMPAT_ASYNC_COMMIT: u32 = 0x0004;
pub const JBD_INCOMPAT_CSUM_V2: u32 = 0x0008;
pub const JBD_INCOMPAT_CSUM_V3: u32 = 0x0010;
pub const JBD_INCOMPAT_FAST_COMMIT: u32 = 0x0020;

/// `s_checksum_type`: JBD2 numbers crc32c fourth, after crc32, md5, and sha1,
/// and e2fsprogs refuses a `CSUM_V3` journal with anything else
pub const JBD2_CRC32C_CHKSUM: u8 = 4;

pub const JBD_INCOMPAT_SUPP: u32 = JBD_INCOMPAT_REVOKE | JBD_INCOMPAT_64BIT | JBD_INCOMPAT_CSUM_V3;

#[derive(Clone, Copy)]
pub struct JournalSuperblock {
    pub data: [u8; 1024],
}

impl JournalSuperblock {
    pub const fn zeroed() -> Self { Self { data: [0; 1024] } }

    fn read_be32(&self, offset: usize) -> u32 {
        u32::from_be_bytes([
            self.data[offset], self.data[offset+1],
            self.data[offset+2], self.data[offset+3],
        ])
    }

    pub fn write_be32(&mut self, offset: usize, val: u32) {
        self.data[offset..offset+4].copy_from_slice(&val.to_be_bytes());
    }

    pub fn magic(&self) -> u32 { self.read_be32(0) }
    pub fn blocktype(&self) -> u32 { self.read_be32(4) }
    pub fn blocksize(&self) -> u32 { self.read_be32(12) }
    pub fn maxlen(&self) -> u32 { self.read_be32(16) }
    pub fn first(&self) -> u32 { self.read_be32(20) }
    pub fn start_sequence(&self) -> u32 { self.read_be32(24) }
    pub fn start(&self) -> u32 { self.read_be32(28) }
    pub fn errno_val(&self) -> i32 { self.read_be32(32) as i32 }
    pub fn feature_compat(&self) -> u32 { self.read_be32(36) }
    pub fn feature_incompat(&self) -> u32 { self.read_be32(40) }
    pub fn feature_ro_compat(&self) -> u32 { self.read_be32(44) }
    pub fn uuid(&self) -> &[u8] { &self.data[48..64] }
    pub fn checksum_type(&self) -> u8 { self.data[80] }
    pub fn is_valid(&self) -> bool { self.magic() == JBD_MAGIC }
    pub fn is_clean(&self) -> bool { self.start() == 0 }
    pub fn is_v2(&self) -> bool { self.blocktype() == JBD_SUPERBLOCK_V2 }

    /// Incompat bits not implemented. `mke2fs` journals have zero
    /// `s_feature_incompat`; `CSUM_V3` tags are sixteen bytes and `64BIT`
    /// twelve, so a wrong stride would read block numbers from other fields and
    /// write the log's contents to them. An unreadable log is refused
    pub fn unsupported_incompat(&self) -> u32 {
        self.feature_incompat() & !JBD_INCOMPAT_SUPP
    }

    /// Bytes per `journal_block_tag` in a descriptor. `journal_block_tag3_t` is
    /// four big-endian words: block, flags, block high, checksum. Without
    /// `CSUM_V3` it is `{block, checksum:16, flags:16}`, with a fourth word for
    /// the block's high half only on a 64-bit journal
    pub fn tag_size(&self) -> usize {
        if self.feature_incompat() & JBD_INCOMPAT_CSUM_V3 != 0 {
            16
        } else if self.feature_incompat() & JBD_INCOMPAT_64BIT != 0 {
            12
        } else {
            8
        }
    }

    /// Where a tag keeps its flags, and how wide. The v3 tag has a word for
    /// them; the older shares it with a 16-bit checksum, flags in the low half
    pub fn tag_flags(&self, buf: &[u8], at: usize) -> u32 {
        if self.is_csum_v3() {
            u32::from_be_bytes([buf[at + 4], buf[at + 5], buf[at + 6], buf[at + 7]])
        } else {
            u16::from_be_bytes([buf[at + 6], buf[at + 7]]) as u32
        }
    }

    /// High half of a tag's block number, only on a 64-bit journal. Such a
    /// block is unaddressable here, so a non-zero value stops the caller
    pub fn tag_block_high(&self, buf: &[u8], at: usize) -> u32 {
        if self.is_csum_v3() {
            u32::from_be_bytes([buf[at + 8], buf[at + 9], buf[at + 10], buf[at + 11]])
        } else if self.feature_incompat() & JBD_INCOMPAT_64BIT != 0 {
            u32::from_be_bytes([buf[at + 8], buf[at + 9], buf[at + 10], buf[at + 11]])
        } else {
            0
        }
    }

    /// Checksum a tag carries for its block. Only the v3 tag has one
    pub fn tag_checksum(&self, buf: &[u8], at: usize) -> u32 {
        if self.is_csum_v3() {
            u32::from_be_bytes([buf[at + 12], buf[at + 13], buf[at + 14], buf[at + 15]])
        } else {
            0
        }
    }

    pub fn is_csum_v3(&self) -> bool {
        self.feature_incompat() & JBD_INCOMPAT_CSUM_V3 != 0
    }

    /// Whether this journal's records carry checksums we can check
    pub fn has_block_checksums(&self) -> bool {
        self.is_csum_v3() && self.checksum_type() == JBD2_CRC32C_CHKSUM
    }
}

#[derive(Clone, Copy)]
pub struct JournalHeader {
    pub magic: u32,
    pub blocktype: u32,
    pub sequence: u32,
}

impl JournalHeader {
    pub fn from_buf(buf: &[u8]) -> Self {
        Self {
            magic:     u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]),
            blocktype: u32::from_be_bytes([buf[4], buf[5], buf[6], buf[7]]),
            sequence:  u32::from_be_bytes([buf[8], buf[9], buf[10], buf[11]]),
        }
    }

    pub fn is_valid(&self) -> bool { self.magic == JBD_MAGIC }
    pub fn is_descriptor(&self) -> bool { self.blocktype == JBD_DESCRIPTOR_BLOCK }
    pub fn is_commit(&self) -> bool { self.blocktype == JBD_COMMIT_BLOCK }
}

#[derive(Clone, Copy)]
pub struct JournalTransaction {
    pub sequence: u32,
    pub start_block: u32,
    pub data_blocks: u32,
    pub committed: bool,
    pub active: bool,
}

impl JournalTransaction {
    pub const fn empty() -> Self {
        Self { sequence: 0, start_block: 0, data_blocks: 0, committed: false, active: false }
    }
}

pub struct JournalInfo {
    pub valid: bool,
    pub version: u8,
    pub block_size: u32,
    pub total_blocks: u32,
    pub first_block: u32,
    pub start: u32,
    pub sequence: u32,
    pub clean: bool,
    pub errno: i32,
    pub transactions: [JournalTransaction; 32],
    pub transaction_count: usize,
    pub journal_inode: u32,
    pub journal_size: u64,
}

impl JournalInfo {
    pub const fn empty() -> Self {
        Self {
            valid: false, version: 0, block_size: 0, total_blocks: 0,
            first_block: 0, start: 0, sequence: 0, clean: false, errno: 0,
            transactions: [JournalTransaction::empty(); 32],
            transaction_count: 0, journal_inode: 0, journal_size: 0,
        }
    }
}

impl<H: ExtHost> MikuFS<H> {
    pub fn has_journal(&self) -> bool {
        self.superblock.has_journal()
    }

    pub fn journal_block_to_disk(&mut self, journal_block: u32) -> Result<u32, FsError> {
        let journal_inode = match self.journal_inode_cached {
            Some(ino) => ino,
            None => {
                let ino = self.read_inode(EXT2_JOURNAL_INO)?;
                self.journal_inode_cached = Some(ino);
                ino
            }
        };
        self.get_file_block(&journal_inode, journal_block)
    }

    pub fn read_journal_superblock(&mut self) -> Result<JournalSuperblock, FsError> {
        if !self.has_journal() {
            return Err(FsError::NoJournal);
        }
        let disk_block = self.journal_block_to_disk(0)?;
        if disk_block == 0 {
            return Err(FsError::CorruptedFs);
        }
        let mut jsb = JournalSuperblock::zeroed();
        let bs = self.block_size as usize;
        let mut buf = [0u8; 4096];
        self.read_block_into(disk_block, &mut buf[..bs])?;
        let copy_size = bs.min(1024);
        jsb.data[..copy_size].copy_from_slice(&buf[..copy_size]);
        if !jsb.is_valid() {
            return Err(FsError::CorruptedFs);
        }
        Ok(jsb)
    }

    pub fn read_journal_block_data(
        &mut self,
        journal_block: u32,
        buf: &mut [u8],
    ) -> Result<(), FsError> {
        let disk_block = self.journal_block_to_disk(journal_block)?;
        if disk_block == 0 {
            return Err(FsError::CorruptedFs);
        }
        self.read_block_into(disk_block, buf)
    }

    pub fn init_journal(&mut self) -> Result<(), FsError> {
        if !self.has_journal() {
            self.journal_active = false;
            return Ok(());
        }
        let j_inode = self.read_inode(EXT2_JOURNAL_INO)?;
        self.journal_inode_cached = Some(j_inode);

        let jsb = self.read_journal_superblock()?;

        // The journal superblock is on-disk data, and these three fields index
        // the log. `maxlen` beyond the journal inode's size walks replay off
        // its end; `first` at or past `maxlen` keeps `advance_journal_pos` from
        // ending the ring
        let bs = self.block_size as u64;
        let inode_blocks = if bs == 0 { 0 } else { j_inode.size().div_ceil(bs) as u32 };
        let maxlen = jsb.maxlen();
        let first = jsb.first();
        if maxlen < 4 || (inode_blocks > 0 && maxlen > inode_blocks) || first == 0 || first >= maxlen
        {
            crate::extlog!(
                "[ext3] journal superblock is not usable (first={} maxlen={} inode holds {}) \
                 - continuing without a journal",
                first, maxlen, inode_blocks
            );
            self.journal_active = false;
            return Ok(());
        }

        // A journal with features that cannot be walked is not written either
        let unsupported = jsb.unsupported_incompat();
        if unsupported != 0 {
            // Read-only: the log may hold unapplied changes that can be neither
            // applied nor written over
            crate::extlog!(
                "[ext3] journal needs features we do not implement ({:#x}) \
                 - mounting read-only",
                unsupported
            );
            self.journal_active = false;
            self.reader.read_only = true;
            return Ok(());
        }
        self.journal_csum_seed = if jsb.has_block_checksums() {
            Some(crate::ext4::crc32c::jbd2_seed(jsb.uuid()))
        } else {
            None
        };

        self.journal_seq = jsb.start_sequence();
        self.journal_maxlen = maxlen;
        self.journal_first = first;
        self.journal_active = true;
        self.running = None;
        self.committing = None;
        if jsb.is_clean() {
            self.journal_pos = jsb.first();
        } else {
            self.journal_pos = jsb.start();
        }
        crate::extlog!(
            "[ext3] journal init: seq={} pos={} max={} active=true",
            self.journal_seq, self.journal_pos, self.journal_maxlen
        );
        Ok(())
    }

    pub fn advance_journal_pos(&self, pos: u32) -> u32 {
        let next = pos + 1;
        if next >= self.journal_maxlen { self.journal_first } else { next }
    }

    /// Joins the running transaction, opening one if none. Nested: every
    /// `begin` has a `commit`, and only the outermost writes, so a compound
    /// operation is one atomic unit
    pub fn ext3_begin_txn(&mut self) -> Result<(), FsError> {
        if !self.journal_active { return Ok(()); }

        if let Some(ref mut t) = self.running {
            t.handles += 1;
            return Ok(());
        }

        self.journal_checkpoint_if_needed()?;
        let desc_pos = self.journal_pos;
        self.journal_pos = self.advance_journal_pos(self.journal_pos);
        let mut t = Transaction::new(self.journal_seq, desc_pos);
        t.handles = 1;
        self.running = Some(t);
        Ok(())
    }

    /// Logs a block into the running transaction, reserving its journal slot
    /// and pinning it so it cannot reach its real location before the commit
    pub fn ext3_journal_current_block(&mut self, fs_block: u32) -> Result<(), FsError> {
        if !self.journal_active || self.running.is_none() { return Ok(()); }

        // Already logged: the journal holds one copy per block per transaction,
        // the content at commit time
        if let Some(ref t) = self.running {
            if t.tags.iter().any(|tag| tag.fs_block == fs_block) {
                return Ok(());
            }
        }

        // One descriptor block per transaction, so a transaction logs at most
        // the tags it holds
        let tags_now = self.running.as_ref().map(|t| t.tags.len()).unwrap_or(0);
        if tags_now + 1 > self.max_tags_per_descriptor() {
            crate::extlog!(
                "[ext3] transaction filled its descriptor block ({} tags) - committing early",
                tags_now
            );
            self.commit_midway()?;
        }

        // A transaction cannot exceed the log that holds it. Closes this one
        // and starts another
        let would_need = self
            .running
            .as_ref()
            .map(|t| t.journal_blocks() + 1)
            .unwrap_or(0);
        if self.journal_maxlen > 0 && would_need + 2 >= self.journal_maxlen {
            crate::extlog!(
                "[ext3] transaction outgrew the journal ({} of {} blocks) - committing early",
                would_need, self.journal_maxlen
            );
            self.commit_midway()?;
        }

        let pos = self.journal_pos;
        self.journal_pos = self.advance_journal_pos(pos);
        if let Some(ref mut t) = self.running {
            t.tags.push(TxnTag { fs_block, journal_pos: pos });
        }
        if let Some(ref mut c) = self.block_cache {
            c.pin(fs_block);
        }
        Ok(())
    }

    /// Closes one handle: JBD2's `stop_handle`, not a commit. Closing the last
    /// makes the transaction eligible; `commit_due` decides if it is written
    /// now. Many operations share one transaction and one set of journal
    /// records, paying once for the descriptor, revoke record, and
    /// barrier-written commit. Logged blocks are pinned, so this is safe
    pub fn ext3_commit_txn(&mut self) -> Result<(), FsError> {
        if !self.journal_active { return Ok(()); }
        match self.running {
            None => return Ok(()),
            Some(ref mut t) => {
                if t.handles > 1 {
                    t.handles -= 1;
                    return Ok(());
                }
                t.handles = 0;
            }
        }
        if self.commit_due() {
            self.force_commit()
        } else {
            Ok(())
        }
    }

    /// Whether the open transaction should be written now: it reached a quarter
    /// of the journal, or enough time passed
    fn commit_due(&self) -> bool {
        let Some(ref t) = self.running else { return false };
        if t.tags.is_empty() { return false; }

        if self.journal_maxlen > 0 && t.journal_blocks() * 4 >= self.journal_maxlen {
            return true;
        }

        let hz = self.reader.host.tick_hz().max(1) as u64;
        let elapsed = self.reader.host.ticks().wrapping_sub(self.last_commit_ticks);
        elapsed >= hz * COMMIT_INTERVAL_SECS
    }

    /// Writes the running transaction regardless of open handles. Used by the
    /// nesting-aware `ext3_commit_txn`, the periodic flush, and when a
    /// transaction outgrows the log.
    ///
    ///   1. descriptor, write-through
    ///   2. one copy of each logged block, write-through
    ///   3. the revoke record, write-through
    ///   4. the commit record, barrier: on stable media on return
    ///   5. only now release the pins, so checkpointing may begin
    ///
    /// Before step 4 replay discards it as incomplete. From step 4 the change
    /// is durable, as replay puts the logged copies in place.
    pub fn force_commit(&mut self) -> Result<(), FsError> {
        if !self.journal_active { return Ok(()); }
        let Some(txn) = self.running.take() else { return Ok(()) };

        if txn.tags.is_empty() {
            // Nothing logged: nothing to replay, and no sequence number used
            if let Some(ref mut c) = self.block_cache {
                c.unpin_all();
            }
            return Ok(());
        }

        self.committing = Some(txn);
        let result = self.write_committing();

        // This transaction is over either way. On failure the pins are released
        // too, since holding blocks for a transaction that will not commit
        // would wedge the cache
        self.committing = None;
        if let Some(ref mut c) = self.block_cache {
            c.unpin_all();
        }
        result
    }

    fn write_committing(&mut self) -> Result<(), FsError> {
        let bs = self.block_size as usize;
        let (sequence, desc_pos, tags, revokes) = {
            let t = match self.committing {
                Some(ref t) => t,
                None => return Ok(()),
            };
            (t.sequence, t.desc_pos, t.tags.clone(), t.revokes.iter().copied().collect::<alloc::vec::Vec<u32>>())
        };

        // The blocks' checksums go in the descriptor, written before them, so
        // contents are read here first. They are pinned until this commit ends,
        // so this is what gets written
        let seed = self.journal_csum_seed;
        let mut sums = alloc::vec::Vec::with_capacity(tags.len());
        if seed.is_some() {
            let mut buf = [0u8; 4096];
            for tag in tags.iter() {
                self.read_block_into(tag.fs_block, &mut buf[..bs])?;
                sums.push(crate::ext4::crc32c::jbd2_block_csum(
                    seed.unwrap(),
                    sequence,
                    &buf[..bs],
                ));
            }
        }

        // 1. Descriptor: which real block each following journal block belongs
        //    to and, where the journal keeps checksums, what each should hash
        //    to
        let tag_size = if seed.is_some() { 16 } else { 8 };
        // A checksummed descriptor keeps its checksum in the block's last four
        // bytes, so tags stop short of them
        let tag_end = if seed.is_some() { bs - crate::ext4::crc32c::JBD2_TAIL_BYTES } else { bs };
        let mut desc = [0u8; 4096];
        desc[0..4].copy_from_slice(&JBD_MAGIC.to_be_bytes());
        desc[4..8].copy_from_slice(&JBD_DESCRIPTOR_BLOCK.to_be_bytes());
        desc[8..12].copy_from_slice(&sequence.to_be_bytes());
        let mut offset = 12;
        for (i, tag) in tags.iter().enumerate() {
            if offset + tag_size > tag_end { break; }
            let mut flags = JBD_FLAG_SAME_UUID;
            if i == tags.len() - 1 { flags |= JBD_FLAG_LAST_TAG; }
            desc[offset..offset + 4].copy_from_slice(&tag.fs_block.to_be_bytes());
            if tag_size == 16 {
                // journal_block_tag3_t: block, flags, block high, checksum
                desc[offset + 4..offset + 8].copy_from_slice(&flags.to_be_bytes());
                desc[offset + 8..offset + 12].copy_from_slice(&0u32.to_be_bytes());
                desc[offset + 12..offset + 16].copy_from_slice(&sums[i].to_be_bytes());
            } else {
                // The older tag keeps a sixteen-bit checksum in this word's
                // high half and the flags in the low half
                desc[offset + 4..offset + 8].copy_from_slice(&flags.to_be_bytes());
            }
            offset += tag_size;
        }
        if let Some(seed) = seed {
            let csum = crate::ext4::crc32c::jbd2_tail_csum(seed, &desc[..bs]);
            desc[bs - 4..bs].copy_from_slice(&csum.to_be_bytes());
        }
        let desc_disk_block = self.journal_block_to_disk(desc_pos)?;
        self.write_block_direct_nocache(desc_disk_block, &desc[..bs])?;

        // 2. The blocks themselves, as they stand
        for tag in tags.iter() {
            let mut buf = [0u8; 4096];
            self.read_block_into(tag.fs_block, &mut buf[..bs])?;
            let jdb = self.journal_block_to_disk(tag.journal_pos)?;
            if jdb != 0 {
                self.write_block_direct_nocache(jdb, &buf[..bs])?;
            }
        }

        // 3. Revokes, carrying this transaction's sequence so replay knows
        //    which logged copies they suppress
        if !revokes.is_empty() {
            self.write_revoke_record(sequence, &revokes)?;
        }

        // 4. Commit, barrier-written: durable on return, and only then may
        //    anything be checkpointed
        let mut commit = [0u8; 4096];
        commit[0..4].copy_from_slice(&JBD_MAGIC.to_be_bytes());
        commit[4..8].copy_from_slice(&JBD_COMMIT_BLOCK.to_be_bytes());
        commit[8..12].copy_from_slice(&sequence.to_be_bytes());
        if let Some(seed) = seed {
            // Tells a torn commit record from a whole one, so half a
            // transaction is not replayed
            let csum = crate::ext4::crc32c::jbd2_commit_csum(seed, &commit[..bs]);
            let at = crate::ext4::crc32c::JBD2_COMMIT_CSUM_OFF;
            commit[at..at + 4].copy_from_slice(&csum.to_be_bytes());
        }
        let commit_disk_block = self.journal_block_to_disk(self.journal_pos)?;
        self.write_block_barrier_nocache(commit_disk_block, &commit[..bs])?;
        self.journal_pos = self.advance_journal_pos(self.journal_pos);

        self.journal_seq = self.journal_seq.wrapping_add(1);
        self.last_commit_ticks = self.reader.host.ticks();
        self.mark_journal_dirty_fast()?;
        // Set in the filesystem's superblock too. Without this flag e2fsck asks
        // whether to run the journal, and Linux does not replay
        self.set_needs_recovery(true)?;
        Ok(())
    }

    /// Raises or lowers `INCOMPAT_RECOVER` in the filesystem superblock and
    /// writes it to the medium at once
    fn set_needs_recovery(&mut self, on: bool) -> Result<(), FsError> {
        let current = self.superblock.feature_incompat();
        let wanted = if on {
            current | FEATURE_INCOMPAT_RECOVER
        } else {
            current & !FEATURE_INCOMPAT_RECOVER
        };
        if wanted == current {
            return Ok(());
        }
        self.superblock.write_u32(96, wanted);
        self.write_superblock_now()
    }

    /// The log's contents are all in place. Called after the checkpoint: the
    /// journal returns to empty and the filesystem stops advertising recovery,
    /// so later mounts and checkers do not replay applied records
    pub fn mark_journal_checkpointed(&mut self) -> Result<(), FsError> {
        if !self.journal_active || self.running.is_some() || self.committing.is_some() {
            return Ok(());
        }
        let disk_blk = self.journal_block_to_disk(0)?;
        if disk_blk == 0 {
            return Ok(());
        }
        let bs = self.block_size as usize;
        let mut buf = [0u8; 4096];
        self.read_block_into(disk_blk, &mut buf[..bs])?;
        if u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) != JBD_MAGIC {
            return Ok(());
        }
        if u32::from_be_bytes([buf[28], buf[29], buf[30], buf[31]]) == 0 {
            return self.set_needs_recovery(false);
        }
        buf[28..32].copy_from_slice(&0u32.to_be_bytes());
        buf[24..28].copy_from_slice(&self.journal_seq.to_be_bytes());
        self.write_journal_sb(disk_blk, &mut buf)?;
        self.journal_pos = self.journal_first;
        self.set_needs_recovery(false)
    }

    /// Discards the running transaction without writing it, along with its
    /// pins. Writes already in the cache are not undone and will be
    /// checkpointed; there is no undo log, so a failed operation must leave the
    /// filesystem consistent by construction
    pub fn ext3_abort_txn(&mut self) {
        self.running = None;
        self.committing = None;
        if let Some(ref mut c) = self.block_cache {
            c.unpin_all();
        }
    }

    /// Whether a transaction is open. Replaces the old `txn_active` flag
    pub fn txn_active(&self) -> bool {
        self.running.is_some()
    }

    /// Blocks one transaction can log: the tags fitting in a descriptor block,
    /// past its twelve-byte header and, with journal checksums, four bytes of
    /// tail
    pub fn max_tags_per_descriptor(&self) -> usize {
        let bs = self.block_size as usize;
        let (tag_size, tail) = if self.journal_csum_seed.is_some() {
            (16, crate::ext4::crc32c::JBD2_TAIL_BYTES)
        } else {
            (8, 0)
        };
        if bs <= 12 + tail {
            return 1;
        }
        ((bs - 12 - tail) / tag_size).max(1)
    }

    /// Writes the journal superblock, refreshing its checksum. The decision
    /// uses the buffer's own feature bits, since this also writes journals
    /// being created. Written through, as it describes the log
    pub(crate) fn write_journal_sb(
        &mut self,
        disk_blk: u32,
        buf: &mut [u8],
    ) -> Result<(), FsError> {
        if buf.len() >= 1024 {
            let incompat = u32::from_be_bytes([buf[40], buf[41], buf[42], buf[43]]);
            if incompat & JBD_INCOMPAT_CSUM_V3 != 0 && buf[80] == JBD2_CRC32C_CHKSUM {
                let at = crate::ext4::crc32c::JBD2_SB_CSUM_OFF;
                buf[at..at + 4].copy_from_slice(&0u32.to_be_bytes());
                let csum = crate::ext4::crc32c::jbd2_superblock_csum(&buf[..1024]);
                buf[at..at + 4].copy_from_slice(&csum.to_be_bytes());
            }
        }
        let bs = self.block_size as usize;
        self.write_block_data_direct(disk_blk, &buf[..bs])
    }

    /// Records in the journal superblock that the log is in use, and from
    /// where. Written through.
    ///
    /// `s_start` and `s_sequence` name the oldest transaction that may need
    /// replay: one position and its sequence number. `self.journal_seq` has
    /// already advanced past the committing transaction, so it would be one too
    /// high
    fn mark_journal_dirty_fast(&mut self) -> Result<(), FsError> {
        let disk_blk = self.journal_block_to_disk(0)?;
        if disk_blk == 0 { return Err(FsError::CorruptedFs); }
        let bs = self.block_size as usize;
        let mut buf = [0u8; 4096];
        self.read_block_into(disk_blk, &mut buf[..bs])?;
        let (start, sequence) = match self.committing {
            Some(ref t) => (t.desc_pos, t.sequence),
            None => (self.journal_pos, self.journal_seq),
        };
        buf[24..28].copy_from_slice(&sequence.to_be_bytes());
        buf[28..32].copy_from_slice(&start.to_be_bytes());
        self.write_journal_sb(disk_blk, &mut buf)?;
        Ok(())
    }

    pub fn ext3_journal_revoke_block(&mut self, fs_block: u32) -> Result<(), FsError> {
        let Some(ref mut t) = self.running else { return Ok(()) };
        t.revokes.insert(fs_block);
        Ok(())
    }

    /// Commits the running transaction mid-operation and opens the next for the
    /// rest, carrying the handles, so the closing `ext3_commit_txn` calls close
    /// the new one
    pub fn commit_midway(&mut self) -> Result<(), FsError> {
        let handles = self.running.as_ref().map(|t| t.handles);
        self.force_commit()?;
        if let Some(handles) = handles {
            self.ext3_begin_txn()?;
            if let Some(ref mut t) = self.running {
                t.handles = handles;
            }
        }
        Ok(())
    }

    /// Whether the open transaction freed `fs_block`. Until commit, it is the
    /// old owner's after a crash, so it is not reused (`alloc_blocks`) or
    /// written over (`write_data_blocks`)
    pub fn freed_in_running(&self, fs_block: u32) -> bool {
        self.running.as_ref().is_some_and(|t| {
            t.freed.range(..=fs_block).next_back().is_some_and(|(&s, &n)| fs_block - s < n)
        })
    }

    /// `len` blocks from `start` freed by the open transaction: one run, joined
    /// to the previous run when continuous
    pub fn note_freed(&mut self, start: u32, len: u32) {
        let Some(ref mut t) = self.running else { return };
        if len == 0 {
            return;
        }
        if let Some((&s, n)) = t.freed.range_mut(..start).next_back() {
            if s + *n == start {
                *n += len;
                return;
            }
        }
        t.freed.insert(start, len);
    }

    /// Writes this transaction's revoke record. A revoke says not to replay any
    /// logged copy of this block from an earlier transaction, and carries the
    /// issuing sequence number, so a rewrite logged after the revoke still
    /// replays, as with free-then-reallocate
    fn write_revoke_record(&mut self, sequence: u32, revokes: &[u32]) -> Result<(), FsError> {
        if revokes.is_empty() { return Ok(()); }
        let bs = self.block_size as usize;
        let mut data = [0u8; 4096];
        data[0..4].copy_from_slice(&JBD_MAGIC.to_be_bytes());
        data[4..8].copy_from_slice(&JBD_REVOKE_BLOCK.to_be_bytes());
        data[8..12].copy_from_slice(&sequence.to_be_bytes());

        // Capacity: a 16-byte header and 4 per entry, with four more at the
        // block's end in a checksummed journal
        let tail = if self.journal_csum_seed.is_some() {
            crate::ext4::crc32c::JBD2_TAIL_BYTES
        } else {
            0
        };
        let capacity = (bs - 16 - tail) / 4;
        let count = revokes.len().min(capacity);
        if count < revokes.len() {
            crate::extlog!(
                "[ext3] revoke record holds {} of {} entries", count, revokes.len()
            );
        }
        let record_size = 16 + count * 4;
        data[12..16].copy_from_slice(&(record_size as u32).to_be_bytes());
        for (i, blk) in revokes.iter().take(count).enumerate() {
            let off = 16 + i * 4;
            data[off..off + 4].copy_from_slice(&blk.to_be_bytes());
        }

        if let Some(seed) = self.journal_csum_seed {
            let csum = crate::ext4::crc32c::jbd2_tail_csum(seed, &data[..bs]);
            data[bs - 4..bs].copy_from_slice(&csum.to_be_bytes());
        }
        let disk = self.journal_block_to_disk(self.journal_pos)?;
        self.write_block_direct_nocache(disk, &data[..bs])?;
        self.journal_pos = self.advance_journal_pos(self.journal_pos);
        Ok(())
    }

    pub fn ext3_create_journal(&mut self, num_blocks: u32) -> Result<(), FsError> {
        if self.has_journal() {
            return Err(FsError::AlreadyExists);
        }
        if num_blocks < 16 {
            return Err(FsError::NoSpace);
        }
        let free = self.superblock.free_blocks_count();
        if num_blocks + 2 > free {
            return Err(FsError::NoSpace);
        }
        let now = self.get_timestamp();
        let mut j_inode = Inode::zeroed();
        j_inode.data = [0; 256];
        j_inode.set_mode(S_IFREG | 0o600);
        j_inode.set_uid(0);
        j_inode.set_gid(0);
        j_inode.set_atime(now);
        j_inode.set_ctime(now);
        j_inode.set_mtime(now);
        j_inode.set_links_count(1);
        let direct_count = num_blocks.min(12);
        for i in 0..direct_count {
            if i > 0 && i % 64 == 0 { self.sync_dirty_blocks()?; }
            let blk = self.alloc_block(0)?;
            self.zero_block(blk)?;
            j_inode.set_block(i as usize, blk);
        }
        if num_blocks > 12 {
            let ptrs_per_block = self.block_size / 4;
            let indirect_blk = self.alloc_block(0)?;
            self.zero_block(indirect_blk)?;
            j_inode.set_block(12, indirect_blk);
            let remaining = (num_blocks - 12).min(ptrs_per_block);
            for i in 0..remaining {
                if i > 0 && i % 64 == 0 { self.sync_dirty_blocks()?; }
                let blk = self.alloc_block(0)?;
                self.zero_block(blk)?;
                self.write_indirect_entry(indirect_blk, i, blk)?;
            }
            let total_disk_blocks = num_blocks + 1;
            j_inode.set_blocks(total_disk_blocks * (self.block_size / 512));
        } else {
            j_inode.set_blocks(num_blocks * (self.block_size / 512));
        }
        let journal_byte_size = num_blocks * self.block_size;
        j_inode.set_size(journal_byte_size);
        if self.inode_size() >= 128 {
            j_inode.write_u32(108, 0);
        }
        self.write_inode(EXT2_JOURNAL_INO, &j_inode)?;
        self.journal_inode_cached = None;
        self.write_journal_superblock(num_blocks)?;
        let compat = self.superblock.feature_compat();
        self.superblock.write_u32(92, compat | FEATURE_COMPAT_HAS_JOURNAL);
        self.superblock.write_u32(224, EXT2_JOURNAL_INO);
        let fs_uuid = self.superblock.uuid();
        let mut uuid_copy = [0u8; 16];
        uuid_copy.copy_from_slice(fs_uuid);
        self.superblock.data[208..224].copy_from_slice(&uuid_copy);
        self.sync_dirty_blocks()?;
        self.flush_all_dirty_metadata()?;
        self.do_write_superblock_direct()?;
        self.init_journal()?;
        Ok(())
    }

    fn do_write_superblock_direct(&mut self) -> Result<(), FsError> {
        if self.superblock.has_metadata_csum() {
            self.update_superblock_csum();
        }
        let mut s0 = [0u8; 512];
        let mut s1 = [0u8; 512];
        s0.copy_from_slice(&self.superblock.data[0..512]);
        s1.copy_from_slice(&self.superblock.data[512..1024]);
        self.reader.write_sector(2, &s0)?;
        self.reader.write_sector(3, &s1)?;
        self.superblock_dirty = false;
        Ok(())
    }

    fn write_journal_superblock(&mut self, num_blocks: u32) -> Result<(), FsError> {
        let j_inode = self.read_inode(EXT2_JOURNAL_INO)?;
        let first_journal_block = j_inode.block(0);
        if first_journal_block == 0 {
            return Err(FsError::CorruptedFs);
        }
        let mut jsb_data = [0u8; 4096];
        jsb_data[0..4].copy_from_slice(&JBD_MAGIC.to_be_bytes());
        jsb_data[4..8].copy_from_slice(&JBD_SUPERBLOCK_V2.to_be_bytes());
        jsb_data[8..12].copy_from_slice(&0u32.to_be_bytes());
        jsb_data[12..16].copy_from_slice(&self.block_size.to_be_bytes());
        jsb_data[16..20].copy_from_slice(&num_blocks.to_be_bytes());
        jsb_data[20..24].copy_from_slice(&1u32.to_be_bytes());
        jsb_data[24..28].copy_from_slice(&1u32.to_be_bytes());
        jsb_data[28..32].copy_from_slice(&0u32.to_be_bytes());
        jsb_data[32..36].copy_from_slice(&0u32.to_be_bytes());
        let fs_uuid = self.superblock.uuid();
        jsb_data[48..64].copy_from_slice(fs_uuid);
        jsb_data[64..68].copy_from_slice(&1u32.to_be_bytes());
        self.write_journal_sb(first_journal_block, &mut jsb_data)?;
        Ok(())
    }
}
