use super::journal::*;
use crate::structs::*;
use crate::{FsError, MikuFS};
use crate::host::ExtHost;
use crate::ext4::crc32c;

/// Whether `a` is strictly older than `b`, given that sequence numbers wrap:
/// JBD2's `tid_lt`. The signed difference works while they are less than 2^31
/// apart
pub fn seq_before(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) < 0
}

impl<H: ExtHost> MikuFS<H> {
    pub fn ext3_clean_journal(&mut self) -> Result<(), FsError> {
        if !self.has_journal() {
            return Err(FsError::NoJournal);
        }
        let j_inode = self.read_inode(EXT2_JOURNAL_INO)?;
        let disk_blk = self.get_file_block(&j_inode, 0)?;
        if disk_blk == 0 {
            return Err(FsError::CorruptedFs);
        }
        let bs = self.block_size as usize;
        let mut buf = [0u8; 4096];
        self.read_block_into(disk_blk, &mut buf[..bs])?;
        let magic = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]);
        if magic != JBD_MAGIC {
            return Err(FsError::CorruptedFs);
        }
        buf[28..32].copy_from_slice(&0u32.to_be_bytes());
        let seq = u32::from_be_bytes([buf[24], buf[25], buf[26], buf[27]]);
        let new_seq = seq.wrapping_add(1);
        buf[24..28].copy_from_slice(&new_seq.to_be_bytes());
        self.write_journal_sb(disk_blk, &mut buf)?;
        self.journal_pos = self.journal_first;
        self.journal_seq = new_seq;
        crate::extlog!("[ext3] journal cleaned: new_seq={}", new_seq);
        Ok(())
    }

    /// Replays the journal.
    ///
    /// Three passes over one walk: a revoke pass recording which blocks must
    /// not return and from which sequence, a scan pairing each descriptor with
    /// its commit record, and the replay.
    ///
    /// The walk spans the whole journal, positions the commit record by every
    /// tag of a descriptor, and scopes each revoke by its sequence
    pub fn ext3_recover(&mut self) -> Result<u32, FsError> {
        if !self.has_journal() {
            return Err(FsError::NoJournal);
        }
        let jsb = self.read_journal_superblock()?;
        if jsb.is_clean() {
            crate::extlog!("[ext3] journal clean, no recovery needed");
            return Ok(0);
        }

        // A log that cannot be read is refused. The tag layout depends on these
        // bits, and a wrong stride reads block numbers from other fields and
        // writes the log's contents there
        let unsupported = jsb.unsupported_incompat();
        if unsupported != 0 {
            crate::extlog!(
                "[ext3] journal needs features we do not implement ({:#x}) - refusing to replay",
                unsupported
            );
            return Err(FsError::UnsupportedFeature);
        }
        let tag_size = jsb.tag_size();
        // What journal checksums chain from: the journal's UUID, not the
        // filesystem's
        let csum_seed = if jsb.has_block_checksums() {
            Some(crc32c::jbd2_seed(jsb.uuid()))
        } else {
            None
        };

        let maxlen = jsb.maxlen();
        let first = jsb.first();
        let start_seq = jsb.start_sequence();
        let start_block = jsb.start();
        if maxlen < 4 || first == 0 || first >= maxlen || start_block >= maxlen {
            crate::extlog!(
                "[ext3] journal superblock unusable for replay (first={} maxlen={} start={})",
                first, maxlen, start_block
            );
            return Err(FsError::CorruptedFs);
        }

        crate::extlog!(
            "[ext3] recovery: start_block={} start_seq={} maxlen={}",
            start_block, start_seq, maxlen
        );

        // Pass 1: revokes, each with its issuing sequence
        let revokes = self.scan_revokes(start_block, first, maxlen, start_seq)?;

        // Passes 2 and 3: walks the log, replaying each transaction with a
        // present, matching commit record
        let mut block = start_block;
        let mut scanned = 0u32;
        let mut replayed = 0u32;
        let mut complete = true;
        let bs = self.block_size as usize;
        let read_size = bs.min(4096);

        while scanned < maxlen {
            let mut buf = [0u8; 4096];
            if self.read_journal_block_data(block, &mut buf[..read_size]).is_err() {
                complete = false;
                break;
            }

            let header = JournalHeader::from_buf(&buf);
            if !header.is_valid() {
                // End of the log: this was never written as a record
                break;
            }

            // Modular comparison, since sequence numbers wrap
            if seq_before(header.sequence, start_seq) {
                block = self.next_journal_block(block, first, maxlen);
                scanned += 1;
                continue;
            }

            if !header.is_descriptor() {
                block = self.next_journal_block(block, first, maxlen);
                scanned += 1;
                continue;
            }

            let descriptor_seq = header.sequence;

            // The descriptor's checksum is in the block's last four bytes. One
            // that does not verify was torn, making every tag suspect
            if let Some(seed) = csum_seed {
                let stored = u32::from_be_bytes([
                    buf[bs - 4], buf[bs - 3], buf[bs - 2], buf[bs - 1],
                ]);
                if crc32c::jbd2_tail_csum(seed, &buf[..bs]) != stored {
                    crate::extlog!(
                        "[ext3] transaction seq={} has a torn descriptor - end of replay",
                        descriptor_seq
                    );
                    break;
                }
            }

            // Block, its copy's position in the log, and the tag's checksum,
            // zero when the journal keeps none
            let mut tags: alloc::vec::Vec<(u32, u32, u32)> = alloc::vec::Vec::new();
            let mut offset = 12usize;
            let mut data_pos = self.next_journal_block(block, first, maxlen);
            let mut unreadable = false;

            // Tags stop short of the descriptor's own checksum
            let tag_end = if csum_seed.is_some() {
                read_size - crc32c::JBD2_TAIL_BYTES
            } else {
                read_size
            };
            loop {
                if offset + tag_size > tag_end {
                    break;
                }
                let blocknr = u32::from_be_bytes([
                    buf[offset], buf[offset + 1], buf[offset + 2], buf[offset + 3],
                ]);
                let flags = jsb.tag_flags(&buf, offset);
                // A block number past 2^32 is unaddressable, and truncating
                // would replay over another block
                if jsb.tag_block_high(&buf, offset) != 0 {
                    crate::extlog!(
                        "[ext3] transaction seq={} logs a block past 2^32 - stopping",
                        descriptor_seq
                    );
                    unreadable = true;
                    break;
                }
                tags.push((blocknr, data_pos, jsb.tag_checksum(&buf, offset)));
                // Advanced for every tag, positioning the commit record
                data_pos = self.next_journal_block(data_pos, first, maxlen);
                offset += tag_size;
                if flags & JBD_FLAG_SAME_UUID == 0 {
                    offset += 16;
                }
                if flags & JBD_FLAG_LAST_TAG != 0 {
                    break;
                }
            }

            if unreadable {
                break;
            }

            // A revoke record sits between the data blocks and the commit, if
            // issued
            let mut commit_pos = data_pos;
            let mut probe = [0u8; 4096];
            if self.read_journal_block_data(commit_pos, &mut probe[..read_size]).is_ok() {
                let ph = JournalHeader::from_buf(&probe);
                if ph.is_valid()
                    && ph.blocktype == JBD_REVOKE_BLOCK
                    && ph.sequence == descriptor_seq
                {
                    commit_pos = self.next_journal_block(commit_pos, first, maxlen);
                }
            }

            let mut commit_buf = [0u8; 4096];
            let committed = if self
                .read_journal_block_data(commit_pos, &mut commit_buf[..read_size])
                .is_ok()
            {
                let ch = JournalHeader::from_buf(&commit_buf);
                ch.is_valid() && ch.is_commit() && ch.sequence == descriptor_seq
            } else {
                false
            };

            if !committed {
                // An incomplete transaction at the log's head is the normal
                // shape of a crash: everything before is durable, this one
                // never happened. Stops and keeps the log
                crate::extlog!(
                    "[ext3] transaction seq={} has no commit record - end of replay",
                    descriptor_seq
                );
                break;
            }

            // A present commit record may not have reached the medium whole.
            // With journal checksums, a torn commit or a missing data block
            // means this and everything after did not happen
            if let Some(seed) = csum_seed {
                let stored = u32::from_be_bytes([
                    commit_buf[crc32c::JBD2_COMMIT_CSUM_OFF],
                    commit_buf[crc32c::JBD2_COMMIT_CSUM_OFF + 1],
                    commit_buf[crc32c::JBD2_COMMIT_CSUM_OFF + 2],
                    commit_buf[crc32c::JBD2_COMMIT_CSUM_OFF + 3],
                ]);
                if crc32c::jbd2_commit_csum(seed, &commit_buf[..bs]) != stored {
                    crate::extlog!(
                        "[ext3] transaction seq={} has a torn commit record - end of replay",
                        descriptor_seq
                    );
                    break;
                }
                let mut torn = false;
                let mut data = [0u8; 4096];
                for (_, j_pos, want) in tags.iter().copied() {
                    if self.read_journal_block_data(j_pos, &mut data[..read_size]).is_err()
                        || crc32c::jbd2_block_csum(seed, descriptor_seq, &data[..bs]) != want
                    {
                        torn = true;
                        break;
                    }
                }
                if torn {
                    crate::extlog!(
                        "[ext3] transaction seq={} has a logged block that does not verify",
                        descriptor_seq
                    );
                    break;
                }
            }

            crate::extlog!(
                "[ext3] replaying txn seq={} blocks={}", descriptor_seq, tags.len()
            );
            for (fs_block, j_pos, _) in tags.iter().copied() {
                // A revoke suppresses copies logged before it. A later rewrite
                // still replays
                if revokes
                    .iter()
                    .any(|&(blk, seq)| blk == fs_block && seq_before(descriptor_seq, seq))
                {
                    continue;
                }
                let mut data = [0u8; 4096];
                if self.read_journal_block_data(j_pos, &mut data[..read_size]).is_ok() {
                    self.write_block_data(fs_block, &data[..bs])?;
                    replayed += 1;
                }
            }

            block = self.next_journal_block(commit_pos, first, maxlen);
            scanned += tags.len() as u32 + 2;
        }

        // Replayed content is in the write-back cache and must reach the medium
        // before the log is discarded
        self.sync_dirty_blocks()?;
        self.flush_all_dirty_metadata()?;
        self.reader.flush_drive();

        crate::extlog!("[ext3] recovery done: replayed={} blocks", replayed);

        // Discards the log only if the walk reached its end; an aborted scan
        // may leave committed transactions unapplied
        if complete {
            self.ext3_clean_journal()?;
        } else {
            crate::extlog!("[ext3] replay did not reach the end of the log - keeping it");
        }

        Ok(replayed)
    }

    /// Collects every revoke record in the log, each with its issuing sequence
    fn scan_revokes(
        &mut self,
        start_block: u32,
        first: u32,
        maxlen: u32,
        start_seq: u32,
    ) -> Result<alloc::vec::Vec<(u32, u32)>, FsError> {
        let bs = self.block_size as usize;
        let read_size = bs.min(4096);
        let mut out = alloc::vec::Vec::new();
        let mut block = start_block;

        for _ in 0..maxlen {
            let mut buf = [0u8; 4096];
            if self.read_journal_block_data(block, &mut buf[..read_size]).is_err() {
                break;
            }
            let header = JournalHeader::from_buf(&buf);
            if !header.is_valid() {
                break;
            }
            if header.blocktype == JBD_REVOKE_BLOCK && !seq_before(header.sequence, start_seq) {
                let rev_size =
                    u32::from_be_bytes([buf[12], buf[13], buf[14], buf[15]]) as usize;
                let mut roff = 16;
                while roff + 4 <= rev_size && roff + 4 <= read_size {
                    let rblk = u32::from_be_bytes([
                        buf[roff], buf[roff + 1], buf[roff + 2], buf[roff + 3],
                    ]);
                    out.push((rblk, header.sequence));
                    roff += 4;
                }
            }
            block = self.next_journal_block(block, first, maxlen);
        }
        Ok(out)
    }

    pub fn scan_journal(&mut self) -> Result<JournalInfo, FsError> {
        let mut info = JournalInfo::empty();
        if !self.has_journal() {
            return Ok(info);
        }
        let jsb = self.read_journal_superblock()?;
        info.valid = true;
        info.version = if jsb.is_v2() { 2 } else { 1 };
        info.block_size = jsb.blocksize();
        info.total_blocks = jsb.maxlen();
        info.first_block = jsb.first();
        info.start = jsb.start();
        info.sequence = jsb.start_sequence();
        info.clean = jsb.is_clean();
        info.errno = jsb.errno_val();
        info.journal_inode = EXT2_JOURNAL_INO;
        let journal_inode = self.read_inode(EXT2_JOURNAL_INO)?;
        info.journal_size = journal_inode.size();
        if !info.clean && info.start > 0 {
            self.scan_journal_transactions(&jsb, &mut info)?;
        }
        Ok(info)
    }

    fn scan_journal_transactions(
        &mut self,
        jsb: &JournalSuperblock,
        info: &mut JournalInfo,
    ) -> Result<(), FsError> {
        let maxlen = jsb.maxlen();
        let first = jsb.first();
        let mut block = jsb.start();
        let bs = self.block_size as usize;
        let mut buf = [0u8; 4096];
        let read_size = bs.min(4096);
        let max_scan = maxlen.min(512);
        let mut scanned = 0u32;
        let mut current_tx: Option<usize> = None;

        while scanned < max_scan && info.transaction_count < 32 {
            if self
                .read_journal_block_data(block, &mut buf[..read_size])
                .is_err()
            {
                break;
            }
            if read_size < 12 {
                break;
            }
            let header = JournalHeader::from_buf(&buf);
            if !header.is_valid() {
                break;
            }
            match header.blocktype {
                JBD_DESCRIPTOR_BLOCK => {
                    let tx_idx = info.transaction_count;
                    if tx_idx >= 32 {
                        break;
                    }
                    info.transactions[tx_idx].sequence = header.sequence;
                    info.transactions[tx_idx].start_block = block;
                    info.transactions[tx_idx].active = true;
                    current_tx = Some(tx_idx);
                    let data_count = count_descriptor_tags(jsb, &buf[..read_size]);
                    info.transactions[tx_idx].data_blocks = data_count;
                    for _ in 0..data_count {
                        block = self.next_journal_block(block, first, maxlen);
                        scanned += 1;
                        if scanned >= max_scan {
                            break;
                        }
                    }
                }
                JBD_COMMIT_BLOCK => {
                    if let Some(idx) = current_tx {
                        if idx < 32 {
                            info.transactions[idx].committed = true;
                        }
                    }
                    info.transaction_count += 1;
                    current_tx = None;
                }
                JBD_REVOKE_BLOCK => {}
                _ => {
                    break;
                }
            }
            block = self.next_journal_block(block, first, maxlen);
            scanned += 1;
        }
        if current_tx.is_some() && info.transaction_count < 32 {
            info.transaction_count += 1;
        }
        Ok(())
    }



    pub fn next_journal_block(&self, current: u32, first: u32, maxlen: u32) -> u32 {
        let next = current + 1;
        if next >= maxlen {
            first
        } else {
            next
        }
    }
}

/// Data blocks a descriptor describes, at this journal's tag width
fn count_descriptor_tags(jsb: &JournalSuperblock, buf: &[u8]) -> u32 {
    let tag_size = jsb.tag_size();
    let mut offset = 12usize;
    let mut count = 0u32;
    let limit = buf.len();
    loop {
        if offset + tag_size > limit {
            break;
        }
        let flags = jsb.tag_flags(buf, offset);
        count += 1;
        offset += tag_size;
        if flags & JBD_FLAG_SAME_UUID == 0 {
            offset += 16;
        }
        if flags & JBD_FLAG_LAST_TAG != 0 {
            break;
        }
    }
    count
}
